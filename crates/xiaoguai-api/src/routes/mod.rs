//! HTTP route handlers.

pub mod admin;
pub mod hotl;
pub mod mcp;
pub mod scheduler_public;
pub mod sessions;
pub mod usage;

use axum::routing::{delete, get, post};
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
#[allow(clippy::too_many_lines)]
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
        .route(
            "/v1/mcp/marketplace",
            get(crate::marketplace::list_marketplace),
        )
        .route(
            "/v1/mcp/marketplace/install",
            post(crate::marketplace::install_from_marketplace),
        )
        .route("/v1/admin/tenants", get(admin::list_tenants))
        .route("/v1/admin/audit", get(admin::list_audit))
        .route("/v1/admin/audit/verify", get(admin::verify_audit))
        .route("/v1/admin/today", get(admin::list_today))
        .route("/v1/admin/eval/suites", get(admin::list_eval_suites))
        .route("/v1/admin/eval/run", post(admin::run_eval_suite))
        .route(
            "/v1/admin/eval/case-from-session",
            post(admin::eval_case_from_session),
        )
        .route(
            "/v1/admin/scheduler/webhooks/:route_id",
            post(admin::scheduler_webhook),
        )
        .route(
            "/v1/admin/scheduler/jobs/compile",
            post(admin::scheduler_compile_job),
        )
        .route(
            "/v1/admin/scheduler/jobs",
            get(admin::scheduler_list_jobs).post(admin::scheduler_upsert_job),
        )
        // v0.12.x.1: admin-ui Scheduler pane — "Run now" + token CRUD.
        .route(
            "/v1/admin/scheduler/jobs/:id/fire-now",
            post(admin::scheduler_fire_now),
        )
        .route(
            "/v1/admin/scheduler/tokens",
            get(admin::scheduler_list_tokens).post(admin::scheduler_create_token),
        )
        .route(
            "/v1/admin/scheduler/tokens/:token",
            delete(admin::scheduler_revoke_token),
        )
        // v1.1.2: conversation fork.
        .route("/v1/sessions/:id/fork", post(sessions::fork_session))
        // v1.1.1 — token-usage aggregation.
        .route("/v1/usage", get(usage::list_usage))
        // v1.2.3 — HOTL boundary policy admin.
        .route(
            "/v1/hotl/policies",
            get(hotl::list_policies).post(hotl::create_policy),
        )
        .route(
            "/v1/hotl/policies/:id",
            delete(hotl::delete_policy),
        );

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

    // v0.12.x.1: public (token-gated) scheduler webhook. Sits OUTSIDE
    // the bearer/Casbin layer stack — external integrators (GitHub
    // push, Slack events) authenticate via the `X-Xiaoguai-Token`
    // header validated against the per-tenant token table. The
    // existing admin route at `/v1/admin/scheduler/webhooks/:route_id`
    // stays inside the v1 layer for internal callers.
    //
    // Rate limiting is still applied so a flood of bad tokens can't
    // saturate the runner; bearer auth is intentionally skipped.
    let public_v1 = Router::new().route(
        "/v1/scheduler/webhooks/:route_id",
        post(scheduler_public::scheduler_webhook_public),
    );
    let public_v1 = if let Some(limiter) = state.rate_limiter.clone() {
        public_v1.route_layer(axum::middleware::from_fn(move |req, next| {
            let l = limiter.clone();
            async move { rate_limit(l, req, next).await }
        }))
    } else {
        public_v1
    };

    // v0.9.1: optionally publish xiaoguai's Toolbox as an MCP server at
    // `/v1/mcp/serve`. Sits outside the v1 layer stack on purpose —
    // bearer/Casbin/rate-limit are wrong defaults for an MCP server
    // (external agents authenticate via the MCP transport's own auth
    // header). When publishing isn't enabled, we don't mount anything.
    let mcp_serve = if state.mcp_publish_enabled {
        Some(crate::mcp_serve::build_router(state.toolbox.clone()))
    } else {
        None
    };

    let app = public
        .merge(v1)
        .merge(public_v1)
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state);

    match mcp_serve {
        Some(m) => app.merge(m),
        None => app,
    }
}

async fn healthz() -> &'static str {
    "ok"
}
