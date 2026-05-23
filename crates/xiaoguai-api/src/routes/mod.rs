//! HTTP route handlers.

pub mod admin;
pub mod mcp;
pub mod sessions;

use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::auth::require_bearer;
use crate::rate_limit::rate_limit;
use crate::rbac::require_authorized;
use crate::state::AppState;

/// Build the v0.5.5+ router. Layers (outermost → innermost):
///   - tracing (request/response spans)
///   - permissive CORS (origin tightening lands with production auth)
///   - optional bearer-token auth on `/v1/**` when `state.auth = Some(...)`.
///     `/healthz` and `/v1/openapi.json` are always public.
///   - optional Casbin per-route enforcement when `state.authz = Some(...)`.
///     Layer order: auth → rbac → handler so the rbac layer sees `Claims`.
pub fn router(state: AppState) -> Router {
    let public = Router::new().route("/healthz", get(healthz));

    let v1 = Router::new()
        .route(
            "/v1/sessions",
            get(sessions::list_sessions).post(sessions::create_session),
        )
        .route("/v1/sessions/:id", get(sessions::get_session))
        .route(
            "/v1/sessions/:id/messages",
            get(sessions::list_messages).post(sessions::send_message),
        )
        .route("/v1/sessions/:id/cancel", post(sessions::cancel_session))
        .route("/v1/mcp/servers", get(mcp::list_servers))
        .route("/v1/admin/tenants", get(admin::list_tenants));

    // Layer order (inner → outer, since `route_layer` adds outward):
    //   handler → rate_limit → rbac → require_bearer
    // so `require_bearer` runs first and populates Claims, then rbac
    // checks the policy, then rate_limit consumes a token, then the
    // handler runs.
    let v1 = if let Some(limiter) = state.rate_limiter.clone() {
        v1.route_layer(axum::middleware::from_fn(move |req, next| {
            let l = limiter.clone();
            async move { rate_limit(l, req, next).await }
        }))
    } else {
        v1
    };

    let v1 = if let Some(authz) = state.authz.clone() {
        v1.route_layer(axum::middleware::from_fn(move |req, next| {
            let a = authz.clone();
            async move { require_authorized(a, req, next).await }
        }))
    } else {
        v1
    };

    let v1 = if let Some(validator) = state.auth.clone() {
        v1.route_layer(axum::middleware::from_fn(move |req, next| {
            let v = validator.clone();
            async move { require_bearer(v, req, next).await }
        }))
    } else {
        v1
    };

    public
        .merge(v1)
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}
