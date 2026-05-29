//! Auth middleware behaviour: `/v1/**` 401s when no token, accepts a
//! Bearer token via the stub validator, and claims override body identity
//! on `create_session`.

mod common;

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::{
    auth::{Claims, StubValidator, TokenValidator},
    router, AppState, CancelRegistry,
};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

fn build_state_with_auth() -> AppState {
    let sessions = InMemorySessionRepo::arc();
    let messages = InMemoryMessageRepo::arc();
    let backend: Arc<dyn LlmBackend> =
        Arc::new(MockBackend::with_script(vec![ScriptStep::text("noop")]));
    let validator: Arc<dyn TokenValidator> = Arc::new(StubValidator {
        claims: Claims {
            sub: "alice".into(),
            tenant_id: "ten_a".into(),
            roles: vec!["user".into()],
        },
    });
    AppState {
        sessions,
        messages,
        backend,
        toolbox: Arc::new(Toolbox::new()),
        agent_defaults: AgentConfig::new("mock"),
        cancels: Arc::new(CancelRegistry::new()),
        mcp_servers: None,
        auth: Some(validator),
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
        workspace_repository: None,
        skill_proposals: None,
        tenant_settings: None,
        skill_author_gate: None,
        skill_audit: None,
        skills_dir: std::path::PathBuf::new(),
    }
}

async fn body_to_value(body: Body) -> Value {
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn post(uri: &str, body: &Value, token: Option<&str>) -> Request<Body> {
    let mut b = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    b.body(Body::from(body.to_string())).unwrap()
}

#[tokio::test]
async fn missing_token_yields_401_on_v1_routes() {
    let app = router(build_state_with_auth());
    let resp = app
        .oneshot(post(
            "/v1/sessions",
            &json!({"user_id":"u","tenant_id":"t","model":"m"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn healthz_is_public_even_with_auth_enabled() {
    let app = router(build_state_with_auth());
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn valid_token_lets_request_through_and_claims_override_body() {
    let app = router(build_state_with_auth());
    // Body claims spoofed identity; claims from validator must win.
    let resp = app
        .oneshot(post(
            "/v1/sessions",
            &json!({"user_id":"mallory","tenant_id":"ten_evil","model":"m"}),
            Some("any-token-the-stub-accepts"),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v = body_to_value(resp.into_body()).await;
    assert_eq!(v["user_id"], "alice");
    assert_eq!(v["tenant_id"], "ten_a");
}

#[tokio::test]
async fn empty_bearer_token_is_rejected_as_401() {
    let app = router(build_state_with_auth());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/v1/sessions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, "Bearer ")
        .body(Body::from(
            json!({"user_id":"u","tenant_id":"t","model":"m"}).to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
