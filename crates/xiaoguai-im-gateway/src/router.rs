//! Mount IM provider webhooks onto an existing axum `Router`.
//!
//! v0.7.1 wires the full `ReactAgent` pipeline. The flow for an inbound
//! Feishu message:
//!
//!   1. Verify signature + parse → `ImEvent::Message`.
//!   2. Return HTTP 200 immediately (Feishu retries non-2xx within
//!      seconds; we don't want to block the HTTP request on the LLM).
//!   3. Spawn a background task that:
//!      - Runs `ReactAgent::run_to_completion` with the inbound text
//!        as the only user message (stateless single-turn — durable
//!        conversation history backed by PG/Valkey is deferred to a
//!        later slice; v0.7.1 is "real outbound", not "real
//!        conversation memory").
//!      - Picks the last assistant message's `content` as the reply
//!        text. Empty content → a polite "no output produced" stub so
//!        the user gets something rather than silence.
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
use xiaoguai_agent::ReactAgent;
use xiaoguai_api::AppState;
use xiaoguai_llm::{Message as LlmMessage, Role};

use crate::provider::{
    ImEvent, ImProvider, IncomingMessage, OutgoingReply, ProviderError, Webhook,
};

#[derive(Clone)]
pub struct GatewayState {
    pub app: AppState,
    pub feishu: Arc<dyn ImProvider>,
}

/// Helper that wires the canonical Feishu route. Accepts any provider
/// implementing `ImProvider`; the wrapper keeps the gateway crate free
/// of provider-specific deps.
pub fn mount_feishu(app: AppState, feishu: Arc<dyn ImProvider>) -> Router {
    let state = GatewayState { app, feishu };
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

/// Background task: run the `ReactAgent` against the inbound text and
/// push the result back to the IM provider. Exposed (pub) so tests can
/// await it directly without going through axum.
pub async fn run_agent_and_reply(
    state: GatewayState,
    msg: IncomingMessage,
) -> Result<OutgoingReply, ProviderError> {
    let agent = ReactAgent::new(
        state.app.backend.clone(),
        (*state.app.toolbox).clone(),
        state.app.agent_defaults.clone(),
    );
    let history = vec![LlmMessage::user(msg.text.clone())];
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
