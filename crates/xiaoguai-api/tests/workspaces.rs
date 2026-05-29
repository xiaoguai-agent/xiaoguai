//! Integration tests for `/v1/workspaces` — CRUD + scoping (v1.3.x).
//!
//! All tests use [`InMemoryWorkspaceRepository`] so no Postgres is needed.

mod common;

use std::sync::Arc;

use axum::body::Body;
use http_body_util::BodyExt;
use hyper::Request;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::routes::router;
use xiaoguai_api::{AppState, CancelRegistry, InMemoryWorkspaceRepository, WorkspaceRepository};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

fn build_state(workspaces: Option<Arc<dyn WorkspaceRepository>>) -> AppState {
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
        authz: None,
        tenants: None,
        rate_limiter: None,
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
        rate_limit_state: None,
        hotl_policy_store: None,
        hotl_enforcer: None,
        outcome_writer: None,
        outcomes_reader: None,
        skill_packs: None,
        memory_store: None,
        workspace_repository: workspaces,
        skill_proposals: None,
        tenant_settings: None,
        skill_author_gate: None,
        skill_audit: None,
        skills_dir: std::path::PathBuf::new(),
        personas: None,
    }
}

async fn body_json(body: Body) -> Value {
    let bytes = BodyExt::collect(body).await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

// ---------------------------------------------------------------------------
// GET /v1/workspaces — list
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_503_when_unwired() {
    let app = router(build_state(None));
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/v1/workspaces?tenant_id={}", Uuid::new_v4()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 503);
}

#[tokio::test]
async fn list_empty_for_new_tenant() {
    let repo = InMemoryWorkspaceRepository::new();
    let app = router(build_state(Some(repo)));
    let tenant = Uuid::new_v4();
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/v1/workspaces?tenant_id={tenant}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body, json!([]));
}

// ---------------------------------------------------------------------------
// POST /v1/workspaces — create
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_workspace_returns_201() {
    let repo = InMemoryWorkspaceRepository::new();
    let app = router(build_state(Some(repo)));
    let tenant = Uuid::new_v4();
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/workspaces")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "tenant_id": tenant, "name": "engineering" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["name"], "engineering");
    assert_eq!(body["archived"], false);
    assert!(body["id"].is_string());
}

#[tokio::test]
async fn create_duplicate_name_returns_409() {
    let repo = InMemoryWorkspaceRepository::new();
    let tenant = Uuid::new_v4();

    let make_req = || {
        Request::builder()
            .method("POST")
            .uri("/v1/workspaces")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "tenant_id": tenant, "name": "ops" }).to_string(),
            ))
            .unwrap()
    };

    // First create succeeds.
    let app = router(build_state(Some(repo.clone())));
    let r1 = app.oneshot(make_req()).await.unwrap();
    assert_eq!(r1.status(), 201);

    // Second create with same name conflicts.
    let app2 = router(build_state(Some(repo)));
    let r2 = app2.oneshot(make_req()).await.unwrap();
    assert_eq!(r2.status(), 409);
}

// ---------------------------------------------------------------------------
// PUT /v1/workspaces/:id — update
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_workspace_renames() {
    let repo = InMemoryWorkspaceRepository::new();
    let tenant = Uuid::new_v4();

    // Create first.
    let ws: Value = {
        let app = router(build_state(Some(repo.clone())));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/workspaces")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({ "tenant_id": tenant, "name": "alpha" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        body_json(resp.into_body()).await
    };
    let id = ws["id"].as_str().unwrap();

    // Update name.
    let app2 = router(build_state(Some(repo)));
    let resp = app2
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/v1/workspaces/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "name": "beta" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["name"], "beta");
}

#[tokio::test]
async fn update_unknown_id_returns_404() {
    let repo = InMemoryWorkspaceRepository::new();
    let app = router(build_state(Some(repo)));
    let resp = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/v1/workspaces/{}", Uuid::new_v4()))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "name": "nope" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

// ---------------------------------------------------------------------------
// DELETE /v1/workspaces/:id — archive
// ---------------------------------------------------------------------------

#[tokio::test]
async fn archive_workspace_returns_204() {
    let repo = InMemoryWorkspaceRepository::new();
    let tenant = Uuid::new_v4();

    let ws: Value = {
        let app = router(build_state(Some(repo.clone())));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/workspaces")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({ "tenant_id": tenant, "name": "old" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        body_json(resp.into_body()).await
    };
    let id = ws["id"].as_str().unwrap();

    let app2 = router(build_state(Some(repo)));
    let resp = app2
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/v1/workspaces/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
}

#[tokio::test]
async fn archive_unknown_returns_404() {
    let repo = InMemoryWorkspaceRepository::new();
    let app = router(build_state(Some(repo)));
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/v1/workspaces/{}", Uuid::new_v4()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

// ---------------------------------------------------------------------------
// Default workspace seeding
// ---------------------------------------------------------------------------

#[tokio::test]
async fn seed_default_then_list_finds_it() {
    let repo = InMemoryWorkspaceRepository::new();
    let tenant = Uuid::new_v4();
    repo.seed_default(tenant);

    let app = router(build_state(Some(repo)));
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/v1/workspaces?tenant_id={tenant}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = body_json(resp.into_body()).await;
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "default");
}

#[tokio::test]
async fn cannot_archive_default_workspace_returns_400() {
    let repo = InMemoryWorkspaceRepository::new();
    let tenant = Uuid::new_v4();
    repo.seed_default(tenant);

    // Find the default workspace id via list.
    let app = router(build_state(Some(repo.clone())));
    let list_resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/v1/workspaces?tenant_id={tenant}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let list = body_json(list_resp.into_body()).await;
    let id = list[0]["id"].as_str().unwrap().to_string();

    let app2 = router(build_state(Some(repo)));
    let resp = app2
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/v1/workspaces/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // InvalidArgument → 400
    assert_eq!(resp.status(), 400);
}
