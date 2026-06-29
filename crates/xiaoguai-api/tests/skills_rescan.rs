//! Integration coverage for Phase 5: `POST /v1/admin/skills/rescan`.
//!
//! Exercises the route end-to-end through the real axum router + `AppState`:
//!   * 503 when no [`PackRescanner`] is wired (a non-`packs` build / no repos);
//!   * 200 `{ "activated": [...] }` when wired, surfacing the slugs the rescan
//!     activated;
//!   * 500 when the rescan reports a backend failure.
//!
//! The handler is store-agnostic (it only depends on the `PackRescanner`
//! trait), so a stub rescanner stands in for the `xiaoguai-core` `SQLite` bridge.
//! The bridge's real behaviour (activating `app-store-reviews`' conversational
//! team into the live persona/team repos, idempotently) is covered by
//! `xiaoguai-core::skills_rescan_bridge` unit tests.

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::skills_rescan::{PackRescanError, PackRescanner};
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

/// A stub rescanner returning a fixed result — stands in for the `SQLite` bridge.
struct StubRescanner(Result<Vec<String>, PackRescanError>);

#[async_trait]
impl PackRescanner for StubRescanner {
    async fn rescan(&self) -> Result<Vec<String>, PackRescanError> {
        self.0.clone()
    }
}

fn build_state(pack_rescanner: Option<Arc<dyn PackRescanner>>) -> AppState {
    let backend: Arc<dyn LlmBackend> =
        Arc::new(MockBackend::with_script(vec![ScriptStep::text("noop")]));
    AppState {
        sessions: InMemorySessionRepo::arc(),
        messages: InMemoryMessageRepo::arc(),
        backend,
        toolbox: Arc::new(Toolbox::new()),
        agent_defaults: AgentConfig::new("mock"),
        cancels: Arc::new(CancelRegistry::new()),
        mcp_servers: None,
        auth: None,
        audit: None,
        audit_verifier: None,
        audit_chain_exporter: None,
        mcp_publish_enabled: false,
        mcp_supervisor: None,
        today: None,
        eval: None,
        webhook_pusher: None,
        nl_job_compiler: None,
        job_upserter: None,
        session_forker: None,
        usage_reader: None,
        webhook_token_validator: None,
        webhook_token_admin: None,
        scheduler_jobs_reader: None,
        hotl_policy_store: None,
        hotl_enforcer: None,
        hotl_decision_store: None,
        hotl_audit: None,
        outcome_writer: None,
        outcomes_reader: None,
        skill_packs: None,
        memory_store: None,
        workspace_repository: None,
        skill_proposals: None,
        tenant_settings: None,
        skill_author_gate: None,
        skill_audit: None,
        skills_dir: std::path::PathBuf::new(),
        personas: None,
        watchers: None,
        loops: None,
        teams: None,
        incidents: None,
        team_audit: None,
        decision_registry: std::sync::Arc::new(
            xiaoguai_api::hotl::decision_registry::DecisionRegistry::new(),
        ),
        pack_rescanner,
        coding_toolbox_factory: None,
    }
}

async fn body_json(body: Body) -> Value {
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn post(uri: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

// ── 503 when no rescanner is wired ───────────────────────────────────────────

#[tokio::test]
async fn rescan_503_when_not_wired() {
    let app = router(build_state(None));
    let resp = app.oneshot(post("/v1/admin/skills/rescan")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ── 200 + activated slugs when wired ─────────────────────────────────────────

#[tokio::test]
async fn rescan_returns_activated_slugs() {
    let rescanner: Arc<dyn PackRescanner> = Arc::new(StubRescanner(Ok(vec![
        "app-store-reviews".to_string(),
        "incident-triage".to_string(),
    ])));
    let app = router(build_state(Some(rescanner)));

    let resp = app.oneshot(post("/v1/admin/skills/rescan")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(
        body["activated"],
        serde_json::json!(["app-store-reviews", "incident-triage"])
    );
}

// ── 200 + empty array when nothing activates ─────────────────────────────────

#[tokio::test]
async fn rescan_empty_when_no_conversational_packs() {
    let rescanner: Arc<dyn PackRescanner> = Arc::new(StubRescanner(Ok(vec![])));
    let app = router(build_state(Some(rescanner)));

    let resp = app.oneshot(post("/v1/admin/skills/rescan")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["activated"], serde_json::json!([]));
}

// ── 500 when the rescan reports a backend failure ────────────────────────────

#[tokio::test]
async fn rescan_500_on_backend_error() {
    let rescanner: Arc<dyn PackRescanner> = Arc::new(StubRescanner(Err(PackRescanError::Backend(
        "db down".into(),
    ))));
    let app = router(build_state(Some(rescanner)));

    let resp = app.oneshot(post("/v1/admin/skills/rescan")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
