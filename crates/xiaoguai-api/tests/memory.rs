//! Integration test for `/v1/memories` mount (sprint-10b S10b-4).
//!
//! The memory routes are already wired in `routes/mod.rs` since v1.3.x but
//! lacked a dedicated integration test. This file closes that gap so the
//! frontend can drop its 404-safe fallback in
//! `frontend/admin-ui/src/panes/Memory.tsx`.
//!
//! Covers:
//!   * `GET /v1/memories?tenant_id=…` returns 200 + `{data: []}` when the
//!     store is empty (was 404 before mounting).
//!   * `POST /v1/memories` + `GET /v1/memories` round-trip.
//!   * `GET /v1/memories` returns 503 when `memory_store` is `None`.

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
use xiaoguai_memory::{InMemoryEmbedder, InMemoryMemoryStore, MemoryStore};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

fn build_state(memory_store: Option<Arc<dyn MemoryStore>>) -> AppState {
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
        memory_store,
        workspace_repository: None,
        skill_proposals: None,
        tenant_settings: None,
        skill_author_gate: None,
        skill_audit: None,
        skills_dir: std::path::PathBuf::new(),
        personas: None,
        watchers: None,
        decision_registry: std::sync::Arc::new(
            xiaoguai_api::hotl::decision_registry::DecisionRegistry::new(),
        ),
    }
}

fn make_store() -> Arc<dyn MemoryStore> {
    let embedder = Arc::new(InMemoryEmbedder::default_dim());
    Arc::new(InMemoryMemoryStore::new(embedder))
}

#[tokio::test]
async fn list_memories_returns_200_when_store_is_empty() {
    let app = router(build_state(Some(make_store())));
    let tenant_id = Uuid::new_v4();
    let req = Request::builder()
        .uri(format!("/v1/memories?tenant_id={tenant_id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "GET /v1/memories must return 200; the frontend client falls back \
         to [] on 404/503 today and this test guards against that regression"
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let data = body
        .get("data")
        .and_then(|v| v.as_array())
        .expect("response shape: {data: []}");
    assert_eq!(data.len(), 0);
}

#[tokio::test]
async fn create_then_list_round_trip() {
    let app = router(build_state(Some(make_store())));
    let tenant_id = Uuid::new_v4();

    let create_body = serde_json::json!({
        "kind": "facts",
        "content": "User prefers concise responses.",
        "tags": ["preference", "communication"],
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/memories")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&create_body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "POST /v1/memories should return 201 with the created memory"
    );

    let req = Request::builder()
        .uri(format!("/v1/memories?tenant_id={tenant_id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let data = body.get("data").and_then(|v| v.as_array()).expect("data");
    assert_eq!(data.len(), 1, "created memory must appear in list");
    assert_eq!(data[0]["kind"], "facts");
    assert_eq!(data[0]["content"], "User prefers concise responses.");
}

#[tokio::test]
async fn list_memories_returns_503_when_store_is_none() {
    let app = router(build_state(None));
    let tenant_id = Uuid::new_v4();
    let req = Request::builder()
        .uri(format!("/v1/memories?tenant_id={tenant_id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}
