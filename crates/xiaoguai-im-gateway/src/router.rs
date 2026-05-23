//! Mount IM provider webhooks onto an existing axum `Router`.
//!
//! v0.7.2 carries an in-memory conversation history keyed by
//! `conversation_id` so subsequent webhook deliveries pick up where
//! the previous turn left off. The flow for an inbound Feishu message:
//!
//!   1. Verify signature + parse → `ImEvent::Message`.
//!   2. Return HTTP 200 immediately (Feishu retries non-2xx within
//!      seconds; we don't want to block the HTTP request on the LLM).
//!   3. Spawn a background task that:
//!      - Snapshots history for `msg.conversation_id`.
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
//!
//! History is held *in-process*. Multi-replica deployments will see
//! split-brain (the chat lands on whichever replica receives the
//! webhook); a Valkey-backed store is the next slice.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::post;
use axum::Router;
use serde_json::json;
use xiaoguai_agent::ReactAgent;
use xiaoguai_api::AppState;
use xiaoguai_llm::{Message as LlmMessage, Role};

use crate::history::ConversationHistory;
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
    pub feishu: Arc<dyn ImProvider>,
    pub history: Arc<ConversationHistory>,
}

/// Helper that wires the canonical Feishu route. Accepts any provider
/// implementing `ImProvider`; the wrapper keeps the gateway crate free
/// of provider-specific deps. Uses [`DEFAULT_HISTORY_TURNS`] for the
/// conversation history window; use [`mount_feishu_with_history`] to
/// supply a tuned `ConversationHistory`.
pub fn mount_feishu(app: AppState, feishu: Arc<dyn ImProvider>) -> Router {
    mount_feishu_with_history(
        app,
        feishu,
        Arc::new(ConversationHistory::new(DEFAULT_HISTORY_TURNS)),
    )
}

/// Like [`mount_feishu`] but lets the caller share a
/// [`ConversationHistory`] across multiple mounts (e.g. when the same
/// process serves Feishu + DingTalk + WeCom).
pub fn mount_feishu_with_history(
    app: AppState,
    feishu: Arc<dyn ImProvider>,
    history: Arc<ConversationHistory>,
) -> Router {
    let state = GatewayState {
        app,
        feishu,
        history,
    };
    Router::new()
        .route("/v1/im/feishu/webhook", post(handle_webhook))
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

    match state.feishu.parse(&webhook).await {
        Ok(ImEvent::Challenge { challenge }) => {
            Json(json!({ "challenge": challenge })).into_response()
        }
        Ok(ImEvent::Message(msg)) => {
            spawn_agent_reply(state, msg);
            (StatusCode::OK, Json(json!({"status":"accepted"}))).into_response()
        }
        Err(ProviderError::BadSignature) => StatusCode::UNAUTHORIZED.into_response(),
        Err(ProviderError::Malformed(msg)) => (StatusCode::BAD_REQUEST, msg).into_response(),
        Err(ProviderError::Transport(msg)) => {
            tracing::error!(%msg, "feishu transport error parsing webhook");
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
/// [`GatewayState::history`] for `msg.conversation_id`.
pub async fn run_agent_and_reply(
    state: GatewayState,
    msg: IncomingMessage,
) -> Result<OutgoingReply, ProviderError> {
    let agent = ReactAgent::new(
        state.app.backend.clone(),
        (*state.app.toolbox).clone(),
        state.app.agent_defaults.clone(),
    );
    // v0.7.2: include the prior turns. Snapshot once so the agent sees
    // a stable view even if another concurrent webhook lands.
    let mut history = state.history.snapshot(&msg.conversation_id);
    let inbound = LlmMessage::user(msg.text.clone());
    history.push(inbound.clone());
    let outcome = agent
        .run_to_completion(history, tokio_util::sync::CancellationToken::new())
        .await
        .map_err(|e| ProviderError::Transport(format!("agent: {e}")))?;
    // Walk messages in reverse so the *latest* assistant text wins, even
    // if the loop also produced earlier tool_calls.
    let reply_text = outcome
        .0
        .messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, Role::Assistant) && !m.content.is_empty())
        .map_or_else(|| "(no reply produced)".to_string(), |m| m.content.clone());
    // Append both turns to history so the *next* webhook sees them.
    state.history.extend(
        &msg.conversation_id,
        [inbound, LlmMessage::assistant(reply_text.clone())],
    );
    let out = OutgoingReply {
        conversation_id: msg.conversation_id.clone(),
        text: reply_text,
    };
    state.feishu.reply(&out).await?;
    Ok(out)
}

fn spawn_agent_reply(state: GatewayState, msg: IncomingMessage) {
    tokio::spawn(async move {
        let conv = msg.conversation_id.clone();
        match run_agent_and_reply(state, msg).await {
            Ok(out) => tracing::info!(chat_id = %conv, len = out.text.len(), "feishu reply sent"),
            Err(err) => tracing::warn!(?err, chat_id = %conv, "feishu reply failed"),
        }
    });
}
