//! Integration coverage for v1.2.3: `GET|POST|DELETE /v1/hotl/policies`.
//!
//! Tests the route → store wire path using `InMemoryHotlPolicyStore`.
//! Budget-logic and enforcer tests live in `xiaoguai-api/src/hotl/`.

mod common;

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::hotl::policy::{
    CreateHotlPolicyRequest, HotlPolicyStore, InMemoryHotlPolicyStore,
};
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

fn build_state(store: Option<Arc<dyn HotlPolicyStore>>) -> AppState {
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
        mcp_publish_enabled: false,
        mcp_supervisor: None,
        today: None,
        eval: None,
        webhook_pusher: None,
        nl_job_compiler: None,
        job_upserter: None,
        usage_reader: None,
        session_forker: None,
        webhook_token_validator: None,
        webhook_token_admin: None,
        scheduler_jobs_reader: None,
        rate_limit_state: None,
        hotl_policy_store: store,
        hotl_enforcer: None,
        outcome_writer: None,
        outcomes_reader: None,
        skill_packs: None,
        memory_store: None,
        workspace_repository: None,
    }
}

async fn body_json(body: Body) -> Value {
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

// ── 503 when not wired ───────────────────────────────────────────────────────

#[tokio::test]
async fn list_503_when_store_not_wired() {
    let app = router(build_state(None));
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/v1/hotl/policies?tenant_id={}", Uuid::new_v4()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn create_503_when_store_not_wired() {
    let app = router(build_state(None));
    let body = serde_json::to_vec(&CreateHotlPolicyRequest {
        tenant_id: Uuid::new_v4(),
        scope: "llm_call".into(),
        window_seconds: 60,
        max_count: Some(5),
        max_usd: None,
        escalate_to: None,
    })
    .unwrap();
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/hotl/policies")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ── CRUD round-trip via HTTP ─────────────────────────────────────────────────

#[tokio::test]
async fn list_empty_for_unknown_tenant() {
    let store: Arc<dyn HotlPolicyStore> = Arc::new(InMemoryHotlPolicyStore::new());
    let app = router(build_state(Some(store)));
    let tid = Uuid::new_v4();
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/v1/hotl/policies?tenant_id={tid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body, serde_json::json!([]));
}

#[tokio::test]
async fn create_returns_201_with_policy() {
    let store: Arc<dyn HotlPolicyStore> = Arc::new(InMemoryHotlPolicyStore::new());
    let app = router(build_state(Some(store)));
    let tid = Uuid::new_v4();

    let req_body = serde_json::to_vec(&CreateHotlPolicyRequest {
        tenant_id: tid,
        scope: "llm_call".into(),
        window_seconds: 3600,
        max_count: Some(100),
        max_usd: Some(5.0),
        escalate_to: Some("ops@example.com".into()),
    })
    .unwrap();

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/hotl/policies")
                .header("content-type", "application/json")
                .body(Body::from(req_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["scope"], "llm_call");
    assert_eq!(body["window_seconds"], 3600);
    assert_eq!(body["max_count"], 100);
    assert!(body["id"].is_string(), "id must be a UUID string");
}

#[tokio::test]
async fn create_then_list_shows_policy() {
    let store: Arc<dyn HotlPolicyStore> = Arc::new(InMemoryHotlPolicyStore::new());
    let app = router(build_state(Some(store)));
    let tid = Uuid::new_v4();

    let req_body = serde_json::to_vec(&CreateHotlPolicyRequest {
        tenant_id: tid,
        scope: "email_send".into(),
        window_seconds: 60,
        max_count: Some(10),
        max_usd: None,
        escalate_to: None,
    })
    .unwrap();

    // POST
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/hotl/policies")
                .header("content-type", "application/json")
                .body(Body::from(req_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // GET
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/v1/hotl/policies?tenant_id={tid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["scope"], "email_send");
}

#[tokio::test]
async fn delete_existing_returns_204() {
    let store: Arc<dyn HotlPolicyStore> = Arc::new(InMemoryHotlPolicyStore::new());
    let app = router(build_state(Some(store)));
    let tid = Uuid::new_v4();

    // Create first.
    let req_body = serde_json::to_vec(&CreateHotlPolicyRequest {
        tenant_id: tid,
        scope: "llm_call".into(),
        window_seconds: 60,
        max_count: Some(5),
        max_usd: None,
        escalate_to: None,
    })
    .unwrap();
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/hotl/policies")
                .header("content-type", "application/json")
                .body(Body::from(req_body))
                .unwrap(),
        )
        .await
        .unwrap();
    let created = body_json(resp.into_body()).await;
    let policy_id = created["id"].as_str().unwrap();

    // Delete.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/v1/hotl/policies/{policy_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // List must be empty now.
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/v1/hotl/policies?tenant_id={tid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_json(resp.into_body()).await;
    assert_eq!(body, serde_json::json!([]));
}

#[tokio::test]
async fn delete_missing_returns_404() {
    let store: Arc<dyn HotlPolicyStore> = Arc::new(InMemoryHotlPolicyStore::new());
    let app = router(build_state(Some(store)));

    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/v1/hotl/policies/{}", Uuid::new_v4()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── validation via POST ──────────────────────────────────────────────────────

#[tokio::test]
async fn create_with_zero_window_returns_400() {
    let store: Arc<dyn HotlPolicyStore> = Arc::new(InMemoryHotlPolicyStore::new());
    let app = router(build_state(Some(store)));

    let bad = serde_json::json!({
        "tenant_id": Uuid::new_v4().to_string(),
        "scope": "llm_call",
        "window_seconds": 0,
        "max_count": 5
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/hotl/policies")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&bad).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_with_no_limits_returns_400() {
    let store: Arc<dyn HotlPolicyStore> = Arc::new(InMemoryHotlPolicyStore::new());
    let app = router(build_state(Some(store)));

    // max_count and max_usd are both omitted.
    let bad = serde_json::json!({
        "tenant_id": Uuid::new_v4().to_string(),
        "scope": "llm_call",
        "window_seconds": 60
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/hotl/policies")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&bad).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ── scope filter on list ─────────────────────────────────────────────────────

#[tokio::test]
async fn list_scope_filter_works() {
    let store: Arc<dyn HotlPolicyStore> = Arc::new(InMemoryHotlPolicyStore::new());
    let app = router(build_state(Some(Arc::clone(&store))));
    let tid = Uuid::new_v4();

    // Create two policies with different scopes.
    for scope in ["llm_call", "email_send"] {
        store
            .create(CreateHotlPolicyRequest {
                tenant_id: tid,
                scope: scope.into(),
                window_seconds: 60,
                max_count: Some(5),
                max_usd: None,
                escalate_to: None,
            })
            .await
            .unwrap();
    }

    // List with scope filter.
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/v1/hotl/policies?tenant_id={tid}&scope=llm_call"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    let arr = body.as_array().unwrap();
    assert_eq!(
        arr.len(),
        1,
        "scope filter must return only llm_call policy"
    );
    assert_eq!(arr[0]["scope"], "llm_call");
}
