//! Sprint-14 S14-1 — `RequireScope<S>` axum extractor.
//!
//! Sprint-13 S13-10 added an inline `claims.scopes.iter().any(...)` check
//! to `routes/hotl_decisions.rs`. Sprint-14 extracts that check into a
//! reusable axum extractor using the **marker-trait** pattern (stable
//! Rust 1.93 does NOT support `&'static str` as a const-generic
//! parameter — see DEC-HLD-018).
//!
//! Tests:
//!
//! 1. `decide_with_scope_returns_201` — operator with `hotl:decide`
//!    succeeds.
//! 2. `decide_without_scope_returns_403_nested_envelope` — operator
//!    without `hotl:decide` gets 403 with the api-contract §1.6 nested
//!    `{error:{code:"scope_required", details:{scope:"hotl:decide"}}}`
//!    envelope (BREAKING vs sprint-13's flat shape).
//! 3. `anonymous_returns_401_before_scope_check` — auth layer runs first,
//!    so a missing bearer token short-circuits at 401.
//! 4. `two_markers_coexist_with_independent_gates` — a tiny test router
//!    builds two routes, one gated by `RequireScope<HotlDecide>` and one
//!    by `RequireScope<HotlPolicyWrite>`, and confirms each rejects with
//!    its own scope name in the response body.

mod common;

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::auth::{Claims, StubValidator, TokenValidator};
use xiaoguai_api::hotl::decision::{HotlDecisionStore, InMemoryHotlDecisionStore};
use xiaoguai_api::hotl::decision_registry::DecisionRegistry;
use xiaoguai_api::middleware::require_scope::{
    HotlDecide, HotlPolicyWrite, RequireScope, ScopeName,
};
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_auth::{Authz, DbPolicyRow};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

// ── helpers ───────────────────────────────────────────────────────────────────

/// The sprint-13 migration-0027 seeded Casbin row. Required so the
/// per-route RBAC layer permits the request through to the handler.
fn seeded_hotl_decide_row() -> DbPolicyRow {
    DbPolicyRow {
        ptype: "p".into(),
        v0: Some("hotl:decide".into()),
        v1: Some("/v1/hotl/decisions".into()),
        v2: Some("POST".into()),
        v3: Some("allow".into()),
        v4: None,
        v5: None,
    }
}

fn claims_with(scopes: Vec<&str>) -> Claims {
    Claims {
        sub: "alice".into(),
        tenant_id: "00000000-0000-0000-0000-000000000abc".into(),
        roles: vec!["system_admin".into()],
        scopes: scopes.into_iter().map(String::from).collect(),
    }
}

fn build_state_with(
    decisions: Arc<dyn HotlDecisionStore>,
    auth: Arc<dyn TokenValidator>,
    authz: Arc<Authz>,
) -> AppState {
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
        auth: Some(auth),
        authz: Some(authz),
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
        usage_reader: None,
        session_forker: None,
        webhook_token_validator: None,
        webhook_token_admin: None,
        scheduler_jobs_reader: None,
        rate_limit_state: None,
        hotl_policy_store: None,
        hotl_enforcer: None,
        hotl_decision_store: Some(decisions),
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
        decision_registry: Arc::new(DecisionRegistry::new()),
    }
}

async fn body_json(body: Body) -> Value {
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Build an `Authz` with the sprint-13 `hotl:decide` rule already merged.
async fn authz_with_decide_rule() -> Arc<Authz> {
    let mut authz = Authz::new_default().await.expect("authz");
    authz
        .merge_db_policies(vec![seeded_hotl_decide_row()])
        .await
        .expect("merge db rule");
    Arc::new(authz)
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// (a) Operator JWT carrying `["hotl:decide"]` succeeds with `201 Created`
/// — proves the `RequireScope<HotlDecide>` extractor lets the request
/// through when the scope is present.
#[tokio::test]
async fn decide_with_scope_returns_201() {
    let authz = authz_with_decide_rule().await;
    let validator: Arc<dyn TokenValidator> = Arc::new(StubValidator {
        claims: claims_with(vec!["hotl:decide"]),
    });
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let app = router(build_state_with(decisions, validator, authz));

    let escalation_id = Uuid::new_v4();
    let body = serde_json::json!({
        "escalation_id": escalation_id.to_string(),
        "verdict": "allow",
        "decided_by": "alice"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/hotl/decisions")
                .header(header::AUTHORIZATION, "Bearer t")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}

/// (b) Operator without `hotl:decide` is rejected with the api-contract
/// §1.6 nested envelope shape:
///
/// ```json
/// {"error":{"code":"scope_required","message":"...","details":{"scope":"hotl:decide"}}}
/// ```
///
/// This is a BREAKING change vs sprint-13's flat
/// `{"error":"forbidden","required_scope":"hotl:decide"}`.
#[tokio::test]
async fn decide_without_scope_returns_403_nested_envelope() {
    let authz = authz_with_decide_rule().await;
    let validator: Arc<dyn TokenValidator> = Arc::new(StubValidator {
        claims: claims_with(vec!["read:audit"]),
    });
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let app = router(build_state_with(decisions, validator, authz));

    let body = serde_json::json!({
        "escalation_id": Uuid::new_v4().to_string(),
        "verdict": "allow",
        "decided_by": "alice"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/hotl/decisions")
                .header(header::AUTHORIZATION, "Bearer t")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let json = body_json(resp.into_body()).await;
    // Nested envelope: error.code + error.details.scope.
    assert_eq!(json["error"]["code"], "scope_required", "json: {json}");
    assert_eq!(
        json["error"]["details"]["scope"], "hotl:decide",
        "json: {json}"
    );
    assert!(
        json["error"]["message"].is_string(),
        "error.message must be a string: {json}"
    );
}

/// (c) Anonymous request (no bearer token) → 401 from the auth layer,
/// which runs BEFORE the scope extractor. Proves the layers compose in
/// the correct order.
#[tokio::test]
async fn anonymous_returns_401_before_scope_check() {
    let authz = authz_with_decide_rule().await;
    let validator: Arc<dyn TokenValidator> = Arc::new(StubValidator {
        claims: claims_with(vec!["hotl:decide"]),
    });
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let app = router(build_state_with(decisions, validator, authz));

    let body = serde_json::json!({
        "escalation_id": Uuid::new_v4().to_string(),
        "verdict": "allow",
        "decided_by": "alice"
    });
    // No Authorization header → 401.
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/hotl/decisions")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "auth layer must run before the scope extractor"
    );
}

// ── (d) two-marker coexistence on a tiny standalone router ────────────────────

/// Handler gated by `RequireScope<HotlDecide>`. Returns 200 + "ok" on
/// success; the extractor itself short-circuits with 403 on miss.
async fn handler_decide(_: RequireScope<HotlDecide>) -> impl IntoResponse {
    "ok"
}

/// Handler gated by `RequireScope<HotlPolicyWrite>`.
async fn handler_policy_write(_: RequireScope<HotlPolicyWrite>) -> impl IntoResponse {
    "ok"
}

/// Build a bare router with two routes — one per marker. Wires the same
/// `require_bearer` middleware the real router uses so `Claims` end up in
/// request extensions.
fn two_marker_router(claims: Claims) -> Router {
    use xiaoguai_api::auth::require_bearer;
    let validator: Arc<dyn TokenValidator> = Arc::new(StubValidator { claims });
    Router::new()
        .route("/decide", post(handler_decide))
        .route("/policy", post(handler_policy_write))
        .route_layer(axum::middleware::from_fn(move |req, next| {
            let v = validator.clone();
            async move { require_bearer(v, req, next).await }
        }))
}

/// (d) Two distinct markers gate two routes independently — each 403
/// names its own scope.
#[tokio::test]
async fn two_markers_coexist_with_independent_gates() {
    // Operator carries ONLY `hotl:decide`. Hitting /decide → 200; hitting
    // /policy → 403 with `scope=hotl:policy:write`.
    let app = two_marker_router(claims_with(vec!["hotl:decide"]));

    let r_decide = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/decide")
                .header(header::AUTHORIZATION, "Bearer t")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r_decide.status(), StatusCode::OK);

    let r_policy = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/policy")
                .header(header::AUTHORIZATION, "Bearer t")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r_policy.status(), StatusCode::FORBIDDEN);
    let json = body_json(r_policy.into_body()).await;
    assert_eq!(json["error"]["code"], "scope_required", "json: {json}");
    assert_eq!(
        json["error"]["details"]["scope"], "hotl:policy:write",
        "the gate that fired must name its OWN scope: {json}"
    );

    // And the marker constants themselves match the wire strings — sanity
    // check so the extractor's failure path can never drift from the
    // type-level marker.
    assert_eq!(HotlDecide::VALUE, "hotl:decide");
    assert_eq!(HotlPolicyWrite::VALUE, "hotl:policy:write");
}
