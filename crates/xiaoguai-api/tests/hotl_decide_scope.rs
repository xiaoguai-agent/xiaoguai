//! Integration coverage for sprint-13 S13-10:
//! Casbin `hotl:decide` scope enforcement on `POST /v1/hotl/decisions`.
//!
//! Sprint-13 migration 0027 seeds `(p, hotl:decide, /v1/hotl/decisions,
//! POST, allow)` into the new `casbin_rule` table. S13-10 wires:
//!
//! 1. a DB-backed merge into the in-memory `Authz` enforcer (hybrid
//!    adapter — CSV remains source of truth, DB rows are additive); and
//! 2. a scope check inside the `create_decision` handler that returns
//!    `403 Forbidden` with `{"error":"forbidden","required_scope":"hotl:decide"}`
//!    when the bearer token's scopes do not include `hotl:decide`.
//!
//! Tests:
//! - `decide_with_scope_returns_201` — operator JWT carrying
//!   `["hotl:decide"]` succeeds.
//! - `decide_without_scope_returns_403` — operator JWT with
//!   `["read:audit"]` only is rejected with the structured body.
//! - `boot_asserts_decide_rule_present` — constructing the enforcer with
//!   the seeded DB row exposes the rule via `Authz::has_policy_rule`.
//!
//! The auth integration uses `StubValidator` to mint deterministic
//! `Claims`; the DB-backed Casbin path uses `Authz::with_db_rules` which
//! takes pre-fetched rows so the test does not need a Postgres fixture.

mod common;

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::auth::{Claims, StubValidator, TokenValidator};
use xiaoguai_api::hotl::decision::{HotlDecisionStore, InMemoryHotlDecisionStore};
use xiaoguai_api::hotl::decision_registry::DecisionRegistry;
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_auth::{Authz, DbPolicyRow};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

// ── helpers ───────────────────────────────────────────────────────────────────

/// The single seeded row migration 0027 inserts. Tests that need a
/// DB-backed enforcer merge this row to mimic the migration.
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

fn claims_with(scopes: Vec<&str>) -> Claims {
    Claims {
        sub: "alice".into(),
        tenant_id: "00000000-0000-0000-0000-000000000abc".into(),
        // `system_admin` matches the path-based default rule so the
        // existing per-route RBAC middleware lets the request through to
        // the handler. Scope check is layered ON TOP of that role check.
        roles: vec!["system_admin".into()],
        scopes: scopes.into_iter().map(String::from).collect(),
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// Operator JWT carrying `["hotl:decide"]` scope succeeds with `201
/// Created`. Uses the hybrid Casbin adapter (CSV + seeded DB row) so
/// the rule is enforceable.
#[tokio::test]
async fn decide_with_scope_returns_201() {
    let mut authz = Authz::new_default().await.expect("authz");
    authz
        .merge_db_policies(vec![seeded_hotl_decide_row()])
        .await
        .expect("merge db rule");
    let authz = Arc::new(authz);

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
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["escalation_id"], escalation_id.to_string());
    assert_eq!(json["verdict"], "allow");
}

/// Operator JWT with `["read:audit"]` only is rejected at the scope
/// gate. Body must include the structured `required_scope` slug so the
/// chat-ui can render a precise error.
#[tokio::test]
async fn decide_without_scope_returns_403() {
    let mut authz = Authz::new_default().await.expect("authz");
    authz
        .merge_db_policies(vec![seeded_hotl_decide_row()])
        .await
        .expect("merge db rule");
    let authz = Arc::new(authz);

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
    assert_eq!(json["error"], "forbidden");
    assert_eq!(json["required_scope"], "hotl:decide");
}

/// Empty scopes (legacy JWT issued before sprint-13 — no `scopes` claim)
/// is also rejected by the scope gate, mirroring the without-scope case.
/// Catches a regression where the handler treats a missing claim as
/// "allow-all".
#[tokio::test]
async fn decide_with_empty_scopes_returns_403() {
    let mut authz = Authz::new_default().await.expect("authz");
    authz
        .merge_db_policies(vec![seeded_hotl_decide_row()])
        .await
        .expect("merge db rule");
    let authz = Arc::new(authz);

    let validator: Arc<dyn TokenValidator> = Arc::new(StubValidator {
        claims: claims_with(vec![]),
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
    assert_eq!(json["required_scope"], "hotl:decide");
}

/// Boot-time defensive assertion: after merging the seeded DB row the
/// enforcer exposes `(p, hotl:decide, /v1/hotl/decisions, POST, allow)`.
/// A partial migration that fails to seed the row would trip this in the
/// real `build_authz` call.
#[tokio::test]
async fn boot_asserts_decide_rule_present() {
    let mut authz = Authz::new_default().await.expect("authz");

    // Pre-merge: rule must NOT be present (the bundled CSV never had it).
    assert!(
        !authz
            .has_policy_rule(&["hotl:decide", "/v1/hotl/decisions", "POST", "allow",])
            .await,
        "pre-merge: hotl:decide rule must be absent from the bundled CSV"
    );

    authz
        .merge_db_policies(vec![seeded_hotl_decide_row()])
        .await
        .expect("merge db rule");

    // Post-merge: rule must be present.
    assert!(
        authz
            .has_policy_rule(&["hotl:decide", "/v1/hotl/decisions", "POST", "allow",])
            .await,
        "post-merge: hotl:decide rule must be present"
    );
}
