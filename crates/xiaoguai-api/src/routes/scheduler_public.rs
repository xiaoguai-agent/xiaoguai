//! v0.12.x.1 — public (token-gated) scheduler webhook route.
//!
//! Lives at `POST /v1/scheduler/webhooks/:route_id` — note: **NOT** under
//! `/admin`. The existing admin route at
//! `/v1/admin/scheduler/webhooks/:route_id` stays put (admin bearer keeps
//! the no-token shortcut for internal callers).
//!
//! Authentication: `X-Xiaoguai-Token` header → `WebhookTokenValidator`.
//! The validator returns `Ok(Some(tenant_id))` on success; the handler
//! then forwards to the same `WebhookPusher` the admin route uses.
//!
//! Status codes (in order of precedence):
//! * 503 — token validator OR webhook pusher unwired
//! * 401 — token missing OR validation returned `None`
//! * 404 — no jobs bound to `route_id` (`delivered == 0`)
//! * 202 — `{ "delivered": N, "tenant_id": "..." }`

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde_json::json;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

const TOKEN_HEADER: &str = "X-Xiaoguai-Token";

pub async fn scheduler_webhook_public(
    State(state): State<AppState>,
    Path(route_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let validator = state.webhook_token_validator.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("scheduler webhook token validator not wired".into())
    })?;
    let pusher = state
        .webhook_pusher
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("scheduler webhook pusher not wired".into()))?;

    let token = headers
        .get(TOKEN_HEADER)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .trim();
    if token.is_empty() {
        return Err(ApiError::missing_webhook_token(format!(
            "{TOKEN_HEADER} header missing or empty"
        )));
    }
    let tenant_id = validator
        .validate(token, &route_id)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("token validate: {e}")))?
        .ok_or_else(|| ApiError::invalid_webhook_token("invalid webhook token for this route"))?;

    let delivered = pusher
        .push(&route_id, body)
        .await
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("webhook push: {e}")))?;
    if delivered == 0 {
        return Err(ApiError::NotFound);
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "delivered": delivered, "tenant_id": tenant_id })),
    ))
}
