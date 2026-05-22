//! HTTP route handlers.

pub mod sessions;

use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

/// Build the v0.5.5 router. Layers (outermost → innermost):
///   - tracing (request/response spans)
///   - permissive CORS (origin tightening lands with auth in v0.5.5.1)
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/sessions", post(sessions::create_session))
        .route("/v1/sessions/:id", get(sessions::get_session))
        .route(
            "/v1/sessions/:id/messages",
            get(sessions::list_messages).post(sessions::send_message),
        )
        .route("/v1/sessions/:id/cancel", post(sessions::cancel_session))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}
