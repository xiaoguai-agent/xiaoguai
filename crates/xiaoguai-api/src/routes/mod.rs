//! HTTP route handlers.

pub mod admin;
pub mod anomaly;
pub mod audit_exports;
pub mod branding;
pub mod experts;
pub mod hotl;
pub mod hotl_decisions;
pub mod incidents;
pub mod loops;
pub mod mcp;
pub mod memory;
pub mod orchestrate;
pub mod outcomes;
pub mod personas;
pub mod providers;
pub mod scheduler_public;
pub mod sessions;
pub mod teams;
pub mod usage;
pub mod watchers;

use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderValue, Method};
use axum::routing::{delete, get, post, put};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::skill_proposals;
use crate::skills;
use crate::skills_rescan;
use crate::workspaces;

use crate::auth::require_auth;
use crate::state::AppState;

/// Build the v0.5.5+ router. Layers (outermost → innermost):
///   - tracing (request/response spans)
///   - CORS restricted to loopback origins (or `XIAOGUAI_CORS_ALLOWED_ORIGINS`)
///     — SEC-06, replacing the old `permissive()` layer
///   - optional username/password (HTTP Basic) auth on `/v1/**` when
///     `state.auth = Some(...)`. `/healthz` is always public. Under DEC-033
///     there is no RBAC layer — every authenticated
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
            put(hotl::update_policy).delete(hotl::delete_policy),
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
        .route(
            "/v1/outcomes",
            get(outcomes::list_outcomes).post(outcomes::record_outcome),
        )
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
        // Phase 5 (skill-pack loader): hot-activate installed conversational
        // pack agent teams without a serve restart. Owner-authed (under
        // /v1/admin/*); 503 when no pack rescanner is wired.
        .route(
            "/v1/admin/skills/rescan",
            post(skills_rescan::rescan_skills),
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
        // T7.2 — bulk import/export (JSONL; static segments, so no clash
        // with the `{id}` route below).
        .route(
            "/v1/memories/export",
            get(memory::export_memories),
        )
        .route(
            "/v1/memories/import",
            // #288: explicit 8 MiB body cap (axum defaults to 2 MiB) — see
            // `memory::IMPORT_BODY_LIMIT_BYTES`. Bigger libraries go through
            // the CLI (`xiaoguai memory import`), not HTTP.
            post(memory::import_memories)
                .layer(DefaultBodyLimit::max(memory::IMPORT_BODY_LIMIT_BYTES)),
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
        // T6 self-healing (GLUE-1) — incident visibility + owner-authed
        // writes for the admin pane. The token-gated ingest POST
        // (`/v1/incidents/ingest/{source}`) lives in `public_v1` below,
        // outside the owner-auth layer; `POST /v1/incidents` here is the
        // owner-authed manual-create path the UI uses instead.
        .route(
            "/v1/incidents",
            get(incidents::list_incidents).post(incidents::create_incident),
        )
        .route("/v1/incidents/{id}", get(incidents::get_incident))
        // T6.3/T6.4 — Analyst (consult) / approval-gated Executor (execute)
        // turns + composed markdown report. Owner-auth like the GETs above.
        .route(
            "/v1/incidents/{id}/analyze",
            post(incidents::analyze_incident),
        )
        .route(
            "/v1/incidents/{id}/approve-repair",
            post(incidents::approve_repair),
        )
        // Owner-authed soft close (admin pane "Dismiss"): any non-terminal
        // incident → `dismissed`.
        .route(
            "/v1/incidents/{id}/dismiss",
            post(incidents::dismiss_incident),
        )
        .route(
            "/v1/incidents/{id}/report",
            get(incidents::incident_report),
        )
        // T3 expert center — deterministic "一句话找专家" suggestion
        // (read-only; user confirms before any attach).
        .route("/v1/experts/suggest", post(experts::suggest_experts))
        // T4 executive orchestration — goal in → members run in parallel →
        // lead synthesizes one answer out, streamed as SSE ExecEvent frames.
        .route(
            "/v1/sessions/{id}/orchestrate",
            post(orchestrate::orchestrate_session),
        )
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
        )
        // Anomaly monitors (xiaoguai-anomaly) — offline back-test of a spec
        // against inline CSV data. `/run` (live KPI eval) returns 503: it needs
        // an external time-series source, deliberately not wired under DEC-033.
        .route("/v1/anomaly/run", post(anomaly::run))
        .route("/v1/anomaly/test", post(anomaly::backtest));

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
        // SEC-11: with no owner auth, the whole `/v1` surface — including the
        // human-approval endpoints `POST /v1/incidents/{{id}}/approve-repair`
        // and `POST /v1/hotl/decisions` — is reachable unauthenticated. Warn
        // loudly (mirrors the /v1/mcp/serve warning). SEC-01 already refuses to
        // start on a non-loopback bind without auth, so this path is loopback.
        tracing::warn!(
            "owner auth is DISABLED — ALL /v1 endpoints, incl. self-healing \
             approval (POST /v1/incidents/{{id}}/approve-repair) and HotL \
             decisions (POST /v1/hotl/decisions), are UNAUTHENTICATED. Set \
             auth.username/password (XIAOGUAI_AUTH__*) before exposing this host."
        );
        v1
    };

    // v0.12.x.1: public (token-gated) scheduler webhook. Sits OUTSIDE the
    // owner-auth layer — external integrators (GitHub push, Slack events)
    // authenticate via the `X-Xiaoguai-Token` header validated against the
    // webhook token table. The admin route at
    // `/v1/admin/scheduler/webhooks/{route_id}` stays inside the v1 layer for
    // internal callers.
    let public_v1 = Router::new()
        .route(
            "/v1/scheduler/webhooks/{route_id}",
            post(scheduler_public::scheduler_webhook_public),
        )
        // T6 self-healing: alert intake from Sentry/Datadog/manual callers.
        // Same out-of-band auth as the scheduler webhook (X-Xiaoguai-Token
        // against the webhook token table, route id "incidents").
        .route(
            "/v1/incidents/ingest/{source}",
            post(incidents::ingest_incident),
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
        .layer(build_cors())
        .with_state(state);

    match mcp_serve {
        Some(m) => app.merge(m),
        None => app,
    }
}

async fn healthz() -> &'static str {
    "ok"
}

/// SEC-06: build the CORS layer. The bundled web UI is served same-origin, so
/// cross-origin access is opt-in. Origins listed in
/// `XIAOGUAI_CORS_ALLOWED_ORIGINS` (comma-separated) are allowed verbatim;
/// otherwise only loopback origins (`localhost` / `127.0.0.1` / `[::1]` on any port)
/// are reflected — enough for the same-origin UI and local dev (vite), while
/// remote sites get no CORS headers and the browser blocks them. Replaces the
/// previous `CorsLayer::permissive()` which echoed any Origin.
fn build_cors() -> CorsLayer {
    use tower_http::cors::{AllowOrigin, Any};
    let base = CorsLayer::new()
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers(Any);
    match std::env::var("XIAOGUAI_CORS_ALLOWED_ORIGINS") {
        Ok(v) if !v.trim().is_empty() => {
            let origins: Vec<HeaderValue> = v
                .split(',')
                .filter_map(|s| HeaderValue::from_str(s.trim()).ok())
                .collect();
            base.allow_origin(origins)
        }
        _ => base.allow_origin(AllowOrigin::predicate(|origin, _parts| {
            origin.to_str().map(is_loopback_origin).unwrap_or(false)
        })),
    }
}

/// True if an `Origin` header value names a loopback host (any scheme/port).
fn is_loopback_origin(origin: &str) -> bool {
    let authority = origin.split_once("://").map_or(origin, |(_, rest)| rest);
    let authority = authority.split('/').next().unwrap_or("");
    // IPv6 origins look like `[::1]:port`; IPv4/hostnames like `host:port`.
    let host = authority.strip_prefix('[').map_or_else(
        || authority.split(':').next().unwrap_or(""),
        |rest| rest.split(']').next().unwrap_or(""),
    );
    // SEC-06 (review fix): parse the host as an IP and use `is_loopback()` —
    // covers all of 127.0.0.0/8 and ::1, and crucially REJECTS suffix-confusion
    // hostnames like `127.evil.com` / `127.0.0.1.attacker.com` (which the old
    // `starts_with("127.")` string check let through). `localhost` is the only
    // non-IP loopback name we honour.
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

#[cfg(test)]
mod cors_tests {
    use super::is_loopback_origin;

    #[test]
    fn loopback_origins_allowed() {
        for o in [
            "http://localhost",
            "http://localhost:5173",
            "http://127.0.0.1:7600",
            "https://127.255.255.254",
            "http://[::1]:8080",
        ] {
            assert!(is_loopback_origin(o), "{o} should be loopback");
        }
    }

    #[test]
    fn suffix_confusion_and_remote_rejected() {
        // SEC-06 regression guard: these must NOT be treated as loopback.
        for o in [
            "http://127.evil.com",
            "http://127.0.0.1.attacker.com",
            "http://localhost.attacker.com",
            "https://evil.com",
            "http://10.0.0.1",
            "http://0.0.0.0",
        ] {
            assert!(!is_loopback_origin(o), "{o} must be rejected");
        }
    }
}
