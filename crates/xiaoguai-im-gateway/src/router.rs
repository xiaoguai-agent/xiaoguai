//! Mount IM provider webhooks onto an existing axum `Router`.
//!
//! v0.7.2 carried an in-memory conversation history keyed by
//! `conversation_id` so subsequent webhook deliveries pick up where the
//! previous turn left off. v0.7.3 generalises the store behind the
//! [`ImHistoryStore`] trait so production deployments can swap in the
//! PG-backed implementation for multi-replica safety.
//!
//! Inbound message flow:
//!
//!   1. Verify signature + parse → `ImEvent::Message`.
//!   2. Return HTTP 200 immediately (Feishu retries non-2xx within
//!      seconds; we don't want to block the HTTP request on the LLM).
//!   3. Spawn a background task that:
//!      - Snapshots history for the conversation (the store may
//!        auto-create tenant/user/session rows under the hood).
//!      - Appends the inbound user message and runs
//!        `ReactAgent::run_to_completion` with the full window.
//!      - Picks the last assistant message's `content` as the reply
//!        text. Empty content → a polite "no output produced" stub.
//!      - Appends the user message + assistant reply to history.
//!      - Calls `provider.reply(...)` with that text.
//!
//! Challenge requests still echo the `challenge` synchronously.
//!
//! HTTP semantics:
//!   - 200 on accepted message (reply runs async)
//!   - 200 on challenge (echoes `{"challenge":"..."}`)
//!   - 401 on signature failure
//!   - 400 on malformed body

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::post;
use axum::Router;
use serde_json::json;
use xiaoguai_api::AppState;
use xiaoguai_llm::Message as LlmMessage;
use xiaoguai_runtime::{run_to_completion, RuntimeContext};

use crate::history::{ConversationHistory, ConversationIdent, ImHistoryStore};
use crate::provider::{
    ImEvent, ImProvider, IncomingMessage, OutgoingReply, ProviderError, Webhook,
};

/// Default sliding-window size for IM conversations. Big enough for a
/// real Q-and-A chain; small enough that ten thousand idle chats won't
/// dominate process memory.
pub const DEFAULT_HISTORY_TURNS: usize = 20;

#[derive(Clone)]
pub struct GatewayState {
    pub app: AppState,
    /// The IM adapter handling this mount. Named generically (was
    /// `feishu` pre-v1.1.3) since the same shape now hosts DingTalk +
    /// WeCom too.
    pub provider: Arc<dyn ImProvider>,
    /// Conversation history store. Default is the in-process
    /// [`ConversationHistory`]; production wires the PG-backed
    /// `PgImHistoryStore`.
    pub history: Arc<dyn ImHistoryStore>,
}

/// Helper that wires the canonical Feishu route. Accepts any provider
/// implementing `ImProvider`; the wrapper keeps the gateway crate free
/// of provider-specific deps. Uses [`DEFAULT_HISTORY_TURNS`] for the
/// conversation history window; use [`mount_feishu_with_history`] to
/// supply a tuned `ImHistoryStore`.
pub fn mount_feishu(app: AppState, feishu: Arc<dyn ImProvider>) -> Router {
    let history: Arc<dyn ImHistoryStore> =
        Arc::new(ConversationHistory::new(DEFAULT_HISTORY_TURNS));
    mount_feishu_with_history(app, feishu, history)
}

/// Like [`mount_feishu`] but lets the caller share an [`ImHistoryStore`]
/// across multiple mounts (e.g. when the same process serves Feishu +
/// DingTalk + WeCom) or substitute a different backend (`PgImHistoryStore`
/// for multi-replica deployments).
pub fn mount_feishu_with_history(
    app: AppState,
    feishu: Arc<dyn ImProvider>,
    history: Arc<dyn ImHistoryStore>,
) -> Router {
    mount_with_history("/v1/im/feishu/webhook", app, feishu, history)
}

/// v1.1.3: DingTalk webhook mount. Same axum-state shape as the Feishu
/// mount — only the URL path differs. Route:
/// `POST /v1/im/dingtalk/webhook`.
pub fn mount_dingtalk(app: AppState, provider: Arc<dyn ImProvider>) -> Router {
    let history: Arc<dyn ImHistoryStore> =
        Arc::new(ConversationHistory::new(DEFAULT_HISTORY_TURNS));
    mount_dingtalk_with_history(app, provider, history)
}

/// v1.1.3: same as [`mount_dingtalk`] but lets the caller share an
/// [`ImHistoryStore`] across mounts (recommended in multi-replica
/// deployments — use [`PgImHistoryStore`] for cross-process safety).
pub fn mount_dingtalk_with_history(
    app: AppState,
    provider: Arc<dyn ImProvider>,
    history: Arc<dyn ImHistoryStore>,
) -> Router {
    mount_with_history("/v1/im/dingtalk/webhook", app, provider, history)
}

/// v1.1.3: WeCom webhook mount. Route: `POST /v1/im/wecom/webhook`.
pub fn mount_wecom(app: AppState, provider: Arc<dyn ImProvider>) -> Router {
    let history: Arc<dyn ImHistoryStore> =
        Arc::new(ConversationHistory::new(DEFAULT_HISTORY_TURNS));
    mount_wecom_with_history(app, provider, history)
}

/// v1.1.3: same as [`mount_wecom`] but accepts a shared
/// [`ImHistoryStore`].
pub fn mount_wecom_with_history(
    app: AppState,
    provider: Arc<dyn ImProvider>,
    history: Arc<dyn ImHistoryStore>,
) -> Router {
    mount_with_history("/v1/im/wecom/webhook", app, provider, history)
}

/// Shared mount implementation. The handler is provider-agnostic — the
/// adapter behind `provider` decides how to verify signatures and
/// parse the body. The only thing each mount picks is the URL path.
fn mount_with_history(
    path: &str,
    app: AppState,
    provider: Arc<dyn ImProvider>,
    history: Arc<dyn ImHistoryStore>,
) -> Router {
    let state = GatewayState {
        app,
        provider,
        history,
    };
    Router::new()
        .route(path, post(handle_webhook))
        .with_state(state)
}

async fn handle_webhook(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let webhook = Webhook {
        headers: headers
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect(),
        body: body.to_vec(),
    };

    match state.provider.parse(&webhook).await {
        Ok(ImEvent::Challenge { challenge }) => {
            Json(json!({ "challenge": challenge })).into_response()
        }
        Ok(ImEvent::Message(msg)) => {
            spawn_agent_reply(state, msg);
            (StatusCode::OK, Json(json!({"status":"accepted"}))).into_response()
        }
        Ok(ImEvent::Ignored) => {
            // Bot message or Slack retry re-delivery — acknowledge and drop.
            (StatusCode::OK, Json(json!({"status":"ignored"}))).into_response()
        }
        Err(ProviderError::BadSignature) => StatusCode::UNAUTHORIZED.into_response(),
        Err(ProviderError::Malformed(msg)) => (StatusCode::BAD_REQUEST, msg).into_response(),
        Err(ProviderError::Transport(msg)) => {
            tracing::error!(%msg, provider = state.provider.name(), "im transport error parsing webhook");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Background task: run the `ReactAgent` against the conversation's
/// accumulated history (including the new inbound message) and push
/// the result back to the IM provider. Exposed (pub) so tests can
/// await it directly without going through axum.
///
/// Side effect: appends `(user, assistant)` turns to
/// [`GatewayState::history`] for the conversation's identity.
pub async fn run_agent_and_reply(
    state: GatewayState,
    msg: IncomingMessage,
) -> Result<OutgoingReply, ProviderError> {
    let ident = ConversationIdent::new(
        msg.provider.clone(),
        msg.tenant_external_id.clone(),
        msg.user_external_id.clone(),
        msg.conversation_id.clone(),
    );
    // v0.6.5: ask the history store for the resolved internal tenant so
    // the agent build picks up per-tenant LlmRouter defaults. PG stores
    // return the synthetic tenant created on first sight; in-memory
    // returns None and we fall back to agent_defaults verbatim.
    let resolved_tenant = state
        .history
        .resolve_tenant(&ident)
        .await
        .map_err(|e| ProviderError::Transport(format!("history resolve tenant: {e}")))?;
    // v0.12.0: route through the shared runtime so REST / IM / scheduler
    // build their agent the same way. Tenant scoping flows through
    // `RuntimeContext::with_tenant`.
    let ctx = RuntimeContext::new(
        state.app.backend.clone(),
        state.app.toolbox.clone(),
        state.app.agent_defaults.clone(),
    )
    .with_tenant(resolved_tenant.clone());

    let prior = state
        .history
        .snapshot(&ident)
        .await
        .map_err(|e| ProviderError::Transport(format!("history snapshot: {e}")))?;
    let inbound = LlmMessage::user(msg.text.clone());
    let mut history = prior;
    history.push(inbound.clone());

    let outcome = run_to_completion(&ctx, history, tokio_util::sync::CancellationToken::new())
        .await
        .map_err(|e| ProviderError::Transport(format!("agent: {e}")))?;

    let reply_text = if outcome.reply_text.is_empty() {
        "(no reply produced)".to_string()
    } else {
        outcome.reply_text.clone()
    };
    // v0.12.0: `new_messages` already slices `[inbound, …assistant turns…]`
    // and falls back to empty when the slide window dropped the inbound.
    // Preserve the v0.7.4 fallback behaviour: empty → minimal pair.
    let to_persist = if outcome.new_messages.is_empty() {
        vec![inbound.clone(), LlmMessage::assistant(reply_text.clone())]
    } else {
        outcome.new_messages.clone()
    };
    state
        .history
        .extend(&ident, to_persist)
        .await
        .map_err(|e| ProviderError::Transport(format!("history extend: {e}")))?;
    let out = OutgoingReply {
        conversation_id: msg.conversation_id.clone(),
        text: reply_text,
    };
    state.provider.reply(&out).await?;
    Ok(out)
}

fn spawn_agent_reply(state: GatewayState, msg: IncomingMessage) {
    let provider_name = state.provider.name();
    tokio::spawn(async move {
        let conv = msg.conversation_id.clone();
        match run_agent_and_reply(state, msg).await {
            Ok(out) => {
                tracing::info!(provider = provider_name, chat_id = %conv, len = out.text.len(), "im reply sent");
            }
            Err(err) => {
                tracing::warn!(provider = provider_name, ?err, chat_id = %conv, "im reply failed");
            }
        }
    });
}
