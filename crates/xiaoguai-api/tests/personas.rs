//! Integration test for `/v1/personas` mount (sprint-10b S10b-1).
//!
//! Boots the production router with an `InMemoryPersonaRepository` behind
//! `AppState.personas` and exercises:
//!   * `GET /v1/personas?tenant_id=…` returns 200 + JSON array (was 404
//!     before mounting).
//!   * `POST /v1/personas` round-trips through to the repository.
//!   * The 503 fallback applies when `personas` is `None`.
//!
//! Auth + RBAC are intentionally off (`auth = None`, `authz = None`) so the
//! test exercises *only* the mount + handler wiring. The rbac.rs test
//! covers the policy side (the bundled policy now grants `tenant_admin`
//! read/write/delete on `/personas/*`).

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};
use xiaoguai_personas::{InMemoryPersonaRepository, PersonaRepository};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

fn build_state(repo: Option<Arc<dyn PersonaRepository>>) -> AppState {
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
        personas: repo,
        watchers: None,
        decision_registry: std::sync::Arc::new(
            xiaoguai_api::hotl::decision_registry::DecisionRegistry::new(),
        ),
    }
}

#[tokio::test]
async fn list_personas_returns_200_with_empty_array_when_repo_is_empty() {
    let repo: Arc<dyn PersonaRepository> = Arc::new(InMemoryPersonaRepository::new());
    let app = router(build_state(Some(repo)));

    let tenant_id = Uuid::new_v4();
    let req = Request::builder()
        .uri(format!("/v1/personas?tenant_id={tenant_id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "GET /v1/personas must return 200 once mounted (was 404 before sprint-10b)"
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        body.is_array(),
        "list endpoint must return a JSON array, got: {body}"
    );
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn create_then_list_round_trip() {
    let repo: Arc<dyn PersonaRepository> = Arc::new(InMemoryPersonaRepository::new());
    let app = router(build_state(Some(repo)));

    let tenant_id = Uuid::new_v4();
    let create_body = serde_json::json!({
        "name": "Support Bot",
        "system_prompt": "You are a helpful support agent.",
        "default_model": null,
        "tool_allowlist": null,
        "escalation_tier": "L1",
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/personas")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&create_body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let req = Request::builder()
        .uri(format!("/v1/personas?tenant_id={tenant_id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let arr = body.as_array().expect("array");
    assert_eq!(arr.len(), 1, "created persona should appear in list");
    assert_eq!(arr[0]["name"], "Support Bot");
}

#[tokio::test]
async fn list_personas_returns_503_when_repo_is_none() {
    let app = router(build_state(None));
    let tenant_id = Uuid::new_v4();
    let req = Request::builder()
        .uri(format!("/v1/personas?tenant_id={tenant_id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}
