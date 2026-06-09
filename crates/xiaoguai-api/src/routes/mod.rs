//! HTTP route handlers.

pub mod admin;
pub mod audit_exports;
pub mod experts;
pub mod hotl;
pub mod hotl_decisions;
pub mod loops;
pub mod mcp;
pub mod memory;
pub mod outcomes;
pub mod personas;
pub mod providers;
pub mod scheduler_public;
pub mod sessions;
pub mod teams;
pub mod usage;
pub mod watchers;

use axum::routing::{delete, get, post, put};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::skill_proposals;
use crate::skills;
use crate::workspaces;

use crate::auth::require_auth;
use crate::state::AppState;

/// Build the v0.5.5+ router. Layers (outermost → innermost):
///   - tracing (request/response spans)
///   - permissive CORS (origin tightening lands with production auth)
///   - optional username/password (HTTP Basic) auth on `/v1/**` when
///     `state.auth = Some(...)`. `/healthz` and `/v1/openapi.json` are always
///     public. Under DEC-033 there is no RBAC layer — every authenticated
///     request is the single static owner.
#[allow(clippy::too_many_lines)]
pub fn router(state: AppState) -> Router {
    let public = Router::new().route("/healthz", get(healthz));

    let v1 = Router::new()
        .route(
            "/v1/sessions",
            get(sessions::list_sessions).post(sessions::create_session),
        )
        .route("/v1/sessions/{id}", get(sessions::get_session))
        .route(
            "/v1/sessions/{id}/messages",
            get(sessions::list_messages).post(sessions::send_message),
        )
        .route("/v1/sessions/{id}/cancel", post(sessions::cancel_session))
        .route("/v1/mcp/servers", get(mcp::list_servers))
        .route(
            "/v1/mcp/marketplace",
            get(crate::marketplace::list_marketplace),
        )
        .route(
            "/v1/mcp/marketplace/install",
            post(crate::marketplace::install_from_marketplace),
        )
        .route("/v1/admin/audit", get(admin::list_audit))
        .route("/v1/admin/audit/verify", get(admin::verify_audit))
        // T5 (Tier-3) — compliance bundle export (SOC2/GDPR/HIPAA).
        .route("/v1/audit/exports", post(audit_exports::export_audit))
        .route("/v1/admin/today", get(admin::list_today))
        .route("/v1/admin/eval/suites", get(admin::list_eval_suites))
        .route("/v1/admin/eval/run", post(admin::run_eval_suite))
        .route(
            "/v1/admin/eval/case-from-session",
            post(admin::eval_case_from_session),
        )
        .route(
            "/v1/admin/scheduler/webhooks/{route_id}",
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
            "/v1/admin/scheduler/jobs/{id}/fire-now",
            post(admin::scheduler_fire_now),
        )
        .route(
            "/v1/admin/scheduler/tokens",
            get(admin::scheduler_list_tokens).post(admin::scheduler_create_token),
        )
        .route(
            "/v1/admin/scheduler/tokens/{token}",
            delete(admin::scheduler_revoke_token),
        )
        // v1.1.2: conversation fork.
        .route("/v1/sessions/{id}/fork", post(sessions::fork_session))
        // v1.1.1 — token-usage aggregation.
        .route("/v1/usage", get(usage::list_usage))
        // /loop L1 (DEC-039) — session-scoped recurring agent turns.
        .route(
            "/v1/loops",
            get(loops::list_loops).post(loops::create_loop),
        )
        .route(
            "/v1/loops/{id}",
            get(loops::get_loop).delete(loops::cancel_loop),
        )
        .route("/v1/loops/{id}/resume", post(loops::resume_loop))
        // v1.2.3 — HOTL boundary policy admin.
        .route(
            "/v1/hotl/policies",
            get(hotl::list_policies).post(hotl::create_policy),
        )
        .route(
            "/v1/hotl/policies/{id}",
            delete(hotl::delete_policy),
        )
        // v1.8.x sprint-11 (S11-3a.1) — HOTL decision-record + raise_policy.
        // 3a.1 does NOT resume any agent loop; response.resumed is always
        // false. Full suspend/resume ships in a later sprint.
        .route(
            "/v1/hotl/decisions",
            post(hotl_decisions::create_decision),
        )
        // Parked-tick visibility (LLD-LOOP-001 §7): operator queue of
        // pending escalations, incl. /loop ticks with no SSE consumer.
        .route("/v1/hotl/pending", get(hotl_decisions::list_pending))
        // v1.2.4 — outcome telemetry (revenue-not-time ROI tracking).
        .route("/v1/outcomes", post(outcomes::record_outcome))
        .route("/v1/outcomes/summary", get(outcomes::outcomes_summary))
        .route("/v1/outcomes/timeseries", get(outcomes::outcomes_timeseries))
        // v1.2.28 — skill pack marketplace.
        .route("/v1/skills/catalog", get(skills::list_catalog))
        .route("/v1/skills/installed", get(skills::list_installed))
        .route("/v1/skills/install", post(skills::install_pack))
        .route(
            "/v1/skills/install/{id}",
            delete(skills::uninstall_pack),
        )
        // v1.5.x — Tier-2 D.1: agent-authored skill proposals.
        .route(
            "/v1/skills/proposals",
            get(skill_proposals::list_proposals),
        )
        .route(
            "/v1/skills/proposals/{id}/approve",
            post(skill_proposals::approve_proposal_handler),
        )
        .route(
            "/v1/skills/proposals/{id}/reject",
            post(skill_proposals::reject_proposal_handler),
        )
        // v1.3.x — long-term memory CRUD + semantic recall.
        .route(
            "/v1/memories",
            get(memory::list_memories).post(memory::create_memory),
        )
        .route(
            "/v1/memories/recall",
            post(memory::recall_memories),
        )
        .route(
            "/v1/memories/similar/{id}",
            get(memory::find_similar),
        )
        .route(
            "/v1/memories/{id}",
            get(memory::get_memory)
                .put(memory::update_memory)
                .delete(memory::delete_memory),
        )
        // v1.3.x — workspaces (above sessions/boards, below tenant).
        .route(
            "/v1/workspaces",
            get(workspaces::list_workspaces).post(workspaces::create_workspace),
        )
        .route(
            "/v1/workspaces/{id}",
            put(workspaces::update_workspace).delete(workspaces::archive_workspace),
        )
        // v1.8.0 (sprint-10b S10b-1) — persona CRUD + session attachment.
        // Personas crate (xiaoguai-personas/src/routes.rs) defined the
        // handlers but never mounted them on the main router; this is the
        // wiring point that unblocks the admin-ui Personas pane.
        .route(
            "/v1/personas",
            get(personas::list_personas).post(personas::create_persona),
        )
        .route(
            "/v1/personas/{id}",
            get(personas::get_persona)
                .patch(personas::update_persona)
                .delete(personas::archive_persona),
        )
        .route(
            "/v1/sessions/{id}/persona",
            get(personas::get_session_persona)
                .put(personas::attach_persona)
                .delete(personas::detach_persona),
        )
        // T3 expert center — team CRUD + session attachment (attach also
        // pins the team's lead persona; see routes::teams docs).
        .route("/v1/teams", get(teams::list_teams).post(teams::create_team))
        .route(
            "/v1/teams/{id}",
            get(teams::get_team)
                .patch(teams::update_team)
                .delete(teams::archive_team),
        )
        .route(
            "/v1/sessions/{id}/team",
            get(teams::get_session_team)
                .put(teams::attach_team)
                .delete(teams::detach_team),
        )
        // T3 expert center — deterministic "一句话找专家" suggestion
        // (read-only; user confirms before any attach).
        .route("/v1/experts/suggest", post(experts::suggest_experts))
        // v1.8.0 (sprint-10b S10b-5) — session-scoped watcher introspection.
        // Matches the URL shape XiaoguaiClient.listSessionWatchers /
        // pauseWatcher / resumeWatcher already calls in frontend/shared.
        // See watchers.rs module docs for why `WatchRunner` is *not*
        // touched in this sprint.
        .route("/v1/watchers", get(watchers::list_watchers))
        .route(
            "/v1/watchers/{id}/pause",
            post(watchers::pause_watcher),
        )
        .route(
            "/v1/watchers/{id}/resume",
            post(watchers::resume_watcher),
        );

    // Layer order (inner → outer, since `route_layer` adds outward):
    //   handler → require_auth
    // so `require_auth` runs first and populates the owner `Claims`, then the
    // handler runs. Under DEC-033 there is no RBAC or rate-limit layer.
    let v1 = if let Some(validator) = state.auth.clone() {
        v1.route_layer(axum::middleware::from_fn(move |req, next| {
            let v = validator.clone();
            async move { require_auth(v, req, next).await }
        }))
    } else {
        v1
    };

    // v0.12.x.1: public (token-gated) scheduler webhook. Sits OUTSIDE the
    // owner-auth layer — external integrators (GitHub push, Slack events)
    // authenticate via the `X-Xiaoguai-Token` header validated against the
    // webhook token table. The admin route at
    // `/v1/admin/scheduler/webhooks/{route_id}` stays inside the v1 layer for
    // internal callers.
    let public_v1 = Router::new().route(
        "/v1/scheduler/webhooks/{route_id}",
        post(scheduler_public::scheduler_webhook_public),
    );

    // v0.9.1: optionally publish xiaoguai's Toolbox as an MCP server at
    // `/v1/mcp/serve`. Off by default (`XIAOGUAI_MCP__PUBLISH`). When enabled it
    // exposes tool execution, so it MUST honour the owner gate: apply the same
    // `require_auth` layer used by `/v1/**`. With no owner auth configured the
    // surface is unauthenticated — emit a loud warning so an operator who opens
    // it knows (previously the comment claimed an MCP-transport auth check that
    // does not exist).
    let mcp_serve = if state.mcp_publish_enabled {
        let m = crate::mcp_serve::build_router(state.toolbox.clone());
        let m = if let Some(validator) = state.auth.clone() {
            m.route_layer(axum::middleware::from_fn(move |req, next| {
                let v = validator.clone();
                async move { require_auth(v, req, next).await }
            }))
        } else {
            tracing::warn!(
                "MCP publishing is ENABLED (/v1/mcp/serve) but no owner auth is configured — \
                 the tool-execution surface is UNAUTHENTICATED. Set auth.username/password \
                 (XIAOGUAI_AUTH__*) before exposing this service."
            );
            m
        };
        Some(m)
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
