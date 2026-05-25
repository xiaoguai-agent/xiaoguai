//! v0.12.x.2 — Integration tests for `WWW-Authenticate` header on 401
//! responses.
//!
//! Covers:
//!   - Webhook route (realm="webhook") — missing token → `invalid_request`,
//!     bad token → `invalid_token`.
//!   - Bearer-protected routes (realm="api" via middleware, which returns a
//!     raw `StatusCode::UNAUTHORIZED` — no WWW-Authenticate from that path;
//!     that path is separate from `ApiError::Unauthorized`).
//!   - `ApiError` unit-level rendering (also in `error.rs` unit tests).
//!   - Existing 200-path tests for the webhook route still pass.

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::{
    router, AppState, CancelRegistry, StaticWebhookTokenValidator, WebhookPushError, WebhookPusher,
};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

// ── stubs ─────────────────────────────────────────────────────────────────

/// Pusher that always delivers 1 job (so the 202 path is reachable).
struct AlwaysOnePusher;

#[async_trait]
impl WebhookPusher for AlwaysOnePusher {
    async fn push(
        &self,
        _route_id: &str,
        _detail: serde_json::Value,
    ) -> Result<usize, WebhookPushError> {
        Ok(1)
    }
}

fn build_state_with_webhook_validator(token: &str, route_id: &str) -> AppState {
    let validator = Arc::new(StaticWebhookTokenValidator {
        token: token.to_string(),
        route_id: route_id.to_string(),
        tenant_id: "ten_a".to_string(),
    });
    let pusher: Arc<dyn WebhookPusher> = Arc::new(AlwaysOnePusher);
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
        webhook_pusher: Some(pusher),
        nl_job_compiler: None,
        job_upserter: None,
        session_forker: None,
        usage_reader: None,
        webhook_token_validator: Some(validator),
        webhook_token_admin: None,
        scheduler_jobs_reader: None,
        rate_limit_state: None,
        hotl_policy_store: None,
        hotl_enforcer: None,
        outcome_writer: None,
        outcomes_reader: None,
    }
}

fn webhook_post(route_id: &str, token: Option<&str>) -> Request<Body> {
    let uri = format!("/v1/scheduler/webhooks/{route_id}");
    let mut b = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(t) = token {
        b = b.header("X-Xiaoguai-Token", t);
    }
    b.body(Body::from(json!({"event": "ping"}).to_string()))
        .unwrap()
}

async fn body_json(body: Body) -> Value {
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

// ── missing token → 401 + WWW-Authenticate with realm="webhook" + invalid_request ──

#[tokio::test]
async fn webhook_missing_token_yields_401_with_www_authenticate() {
    let app = router(build_state_with_webhook_validator("secret-tok", "route1"));
    let resp = app.oneshot(webhook_post("route1", None)).await.unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let hdr = resp
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .expect("WWW-Authenticate must be present on 401")
        .to_str()
        .unwrap();

    assert!(
        hdr.contains(r#"realm="webhook""#),
        "realm must be 'webhook', got: {hdr}"
    );
    assert!(
        hdr.contains(r#"error="invalid_request""#),
        "missing token → invalid_request, got: {hdr}"
    );
}

// ── bad/wrong token → 401 + WWW-Authenticate with realm="webhook" + invalid_token ──

#[tokio::test]
async fn webhook_bad_token_yields_401_with_invalid_token_error() {
    let app = router(build_state_with_webhook_validator("secret-tok", "route1"));
    let resp = app
        .oneshot(webhook_post("route1", Some("wrong-token")))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let hdr = resp
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .expect("WWW-Authenticate must be present on 401")
        .to_str()
        .unwrap();

    assert!(
        hdr.contains(r#"realm="webhook""#),
        "realm must be 'webhook', got: {hdr}"
    );
    assert!(
        hdr.contains(r#"error="invalid_token""#),
        "wrong token → invalid_token, got: {hdr}"
    );
}

// ── correct token → 202 (existing happy-path must still work) ─────────────

#[tokio::test]
async fn webhook_correct_token_yields_202() {
    let app = router(build_state_with_webhook_validator("secret-tok", "route1"));
    let resp = app
        .oneshot(webhook_post("route1", Some("secret-tok")))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    let body = body_json(resp.into_body()).await;
    assert_eq!(body["delivered"], 1);
    assert_eq!(body["tenant_id"], "ten_a");
}

// ── token bound to different route → 401 + invalid_token ─────────────────

#[tokio::test]
async fn webhook_token_wrong_route_yields_401_invalid_token() {
    // Validator is bound to "route1"; we post to "route2".
    let app = router(build_state_with_webhook_validator("secret-tok", "route1"));
    let resp = app
        .oneshot(webhook_post("route2", Some("secret-tok")))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let hdr = resp
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .expect("WWW-Authenticate must be present on 401")
        .to_str()
        .unwrap();

    assert!(hdr.contains(r#"error="invalid_token""#), "got: {hdr}");
    assert!(hdr.contains(r#"realm="webhook""#), "got: {hdr}");
}

// ── 401 response body has expected JSON shape ─────────────────────────────

#[tokio::test]
async fn webhook_401_body_has_code_unauthorized() {
    let app = router(build_state_with_webhook_validator("secret-tok", "route1"));
    let resp = app.oneshot(webhook_post("route1", None)).await.unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["code"], "unauthorized");
}
