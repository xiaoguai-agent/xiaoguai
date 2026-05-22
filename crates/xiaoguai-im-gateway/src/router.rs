//! Mount IM provider webhooks onto an existing axum `Router`.
//!
//! v0.7 supports only Feishu's HTTP shape:
//!   `POST /v1/im/feishu/webhook`
//!
//! The handler:
//!   1. Builds a `Webhook` from request headers + body.
//!   2. Asks the provider to verify + parse.
//!   3. For `ImEvent::Challenge`, echoes the challenge JSON back.
//!   4. For `ImEvent::Message`, spawns a best-effort `reply(...)` (the
//!      real `ReactAgent` run + persistence is the v0.7.1 follow-up; here
//!      we only prove the wiring).
//!
//! Returns HTTP 401 on signature failure, 400 on malformed body, 200 on
//! success.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::post;
use axum::Router;
use serde_json::json;
use xiaoguai_api::AppState;

use crate::provider::{ImEvent, ImProvider, OutgoingReply, ProviderError, Webhook};

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
            // v0.7: kick off a best-effort echo reply so we know the
            // wiring works end-to-end. The full ReactAgent integration
            // (session lookup/create + run_stream + reply with final
            // text) lands in v0.7.1.
            let reply = OutgoingReply {
                conversation_id: msg.conversation_id.clone(),
                text: format!("[xiaoguai stub] received: {}", msg.text),
            };
            let provider = state.feishu.clone();
            tokio::spawn(async move {
                if let Err(err) = provider.reply(&reply).await {
                    tracing::warn!(?err, "feishu reply failed");
                }
            });
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
