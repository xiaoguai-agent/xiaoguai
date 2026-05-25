//! End-to-end test of the Casbin per-route middleware. Goes through the
//! real `router()` so the layer order is the one production uses.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::auth::{Claims, StubValidator, TokenValidator};
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_auth::Authz;
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

async fn build_state(roles: Vec<&str>) -> AppState {
    let validator: Arc<dyn TokenValidator> = Arc::new(StubValidator {
        claims: Claims {
            sub: "u".into(),
            tenant_id: "ten_a".into(),
            roles: roles.into_iter().map(str::to_string).collect(),
        },
    });
    let backend: Arc<dyn LlmBackend> =
        Arc::new(MockBackend::with_script(vec![ScriptStep::text("noop")]));
    let authz = Authz::new_default().await.expect("authz");
    AppState {
        sessions: InMemorySessionRepo::arc(),
        messages: InMemoryMessageRepo::arc(),
        backend,
        toolbox: Arc::new(Toolbox::new()),
        agent_defaults: AgentConfig::new("mock"),
        cancels: Arc::new(CancelRegistry::new()),
        mcp_servers: None,
        auth: Some(validator),
        authz: Some(Arc::new(authz)),
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
        workspace_repository: None,
    }
}

#[tokio::test]
async fn tenant_admin_can_read_session() {
    // /v1/sessions/anything is normalized to /sessions/anything, which the
    // policy line `tenant_admin, *, /sessions/*, read` covers.
    let app = router(build_state(vec!["tenant_admin"]).await);
    let req = Request::builder()
        .uri("/v1/sessions/sess_does_not_exist")
        .header(header::AUTHORIZATION, "Bearer t")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // 404 (session not found) is *after* rbac → it confirms the layer
    // allowed the request through.
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "rbac should have allowed; the handler then returns 404"
    );
}

#[tokio::test]
async fn role_with_no_matching_policy_gets_403() {
    let app = router(build_state(vec!["member"]).await);
    // Members are only allowed on `/sessions/own/*`, not on bare
    // `/sessions/:id`. Should be denied.
    let req = Request::builder()
        .uri("/v1/sessions/sess_x")
        .header(header::AUTHORIZATION, "Bearer t")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn empty_roles_gets_403() {
    let app = router(build_state(vec![]).await);
    let req = Request::builder()
        .uri("/v1/sessions/sess_x")
        .header(header::AUTHORIZATION, "Bearer t")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn system_admin_can_do_anything() {
    let app = router(build_state(vec!["system_admin"]).await);
    // system_admin's grant is (sub=*, dom=*, obj=*, act=*) → unconditional.
    let req = Request::builder()
        .uri("/v1/sessions/sess_x")
        .header(header::AUTHORIZATION, "Bearer t")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn healthz_is_public_even_with_rbac() {
    // /healthz is mounted outside the v1 layer; rbac must not gate it
    // (or otherwise unauthenticated probes would 401 in production).
    let app = router(build_state(vec!["member"]).await);
    let req = Request::builder()
        .uri("/healthz")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
