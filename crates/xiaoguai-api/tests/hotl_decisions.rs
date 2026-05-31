//! Integration coverage for v1.8.x sprint-11 (S11-3a.1):
//! `POST /v1/hotl/decisions`.
//!
//! Tests the route → store wire path using `InMemoryHotlDecisionStore` +
//! `InMemoryHotlPolicyStore` + `InMemoryHotlAuditSink`. The agent loop
//! does not suspend on `Escalate` in this milestone, so every assertion
//! checks `resumed == false`.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use serde_json::Value;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use uuid::Uuid;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::auth::{Claims, StubValidator, TokenValidator};
use xiaoguai_api::hotl::audit::{HotlAuditSink, InMemoryHotlAuditSink};
use xiaoguai_api::hotl::decision::{HotlDecisionStore, InMemoryHotlDecisionStore};
use xiaoguai_api::hotl::decision_registry::{DecisionRegistry, HotlResolution, HotlTicketError};
use xiaoguai_api::hotl::policy::{HotlPolicyStore, InMemoryHotlPolicyStore};
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_auth::Authz;
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

// ── state builders ────────────────────────────────────────────────────────────

#[derive(Default)]
struct StateOptions {
    decision_store: Option<Arc<dyn HotlDecisionStore>>,
    policy_store: Option<Arc<dyn HotlPolicyStore>>,
    audit_sink: Option<Arc<dyn HotlAuditSink>>,
    auth: Option<Arc<dyn TokenValidator>>,
    authz: Option<Arc<Authz>>,
    decision_registry: Option<Arc<DecisionRegistry>>,
}

fn build_state(opts: StateOptions) -> AppState {
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
        auth: opts.auth,
        authz: opts.authz,
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
        hotl_policy_store: opts.policy_store,
        hotl_enforcer: None,
        hotl_decision_store: opts.decision_store,
        hotl_audit: opts.audit_sink,
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
        decision_registry: opts
            .decision_registry
            .unwrap_or_else(|| std::sync::Arc::new(DecisionRegistry::new())),
    }
}

async fn body_json(body: Body) -> Value {
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

// ── 1. 503 when store unwired ────────────────────────────────────────────────

#[tokio::test]
async fn decision_503_when_store_unwired() {
    let app = router(build_state(StateOptions::default()));
    let body = serde_json::json!({
        "request_id": Uuid::new_v4().to_string(),
        "verdict": "allow",
        "decided_by": "alice"
    });
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
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// ── 2. Approve happy path: 201 + resumed:false ───────────────────────────────

#[tokio::test]
async fn approve_happy_path_records_and_returns_201() {
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let app = router(build_state(StateOptions {
        decision_store: Some(Arc::clone(&decisions)),
        ..Default::default()
    }));
    let request_id = Uuid::new_v4();
    let body = serde_json::json!({
        "request_id": request_id.to_string(),
        "verdict": "allow",
        "decided_by": "alice"
    });
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
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["request_id"], request_id.to_string());
    assert_eq!(json["verdict"], "allow");
    assert_eq!(
        json["resumed"], false,
        "3a.1 invariant: resumed must always be false"
    );
    assert!(
        json.get("policy_created").is_none() || json["policy_created"].is_null(),
        "no raise_policy → policy_created omitted"
    );
    assert!(json["id"].is_string());
    assert!(json["recorded_at"].is_string());
}

// ── 3. Deny happy path ───────────────────────────────────────────────────────

#[tokio::test]
async fn deny_happy_path() {
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let app = router(build_state(StateOptions {
        decision_store: Some(Arc::clone(&decisions)),
        ..Default::default()
    }));
    let body = serde_json::json!({
        "request_id": Uuid::new_v4().to_string(),
        "verdict": "deny",
        "decided_by": "bob"
    });
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
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["verdict"], "deny");
    assert_eq!(json["resumed"], false);
}

// ── 4. Approve & remember — raise_policy creates a HotlPolicy atomically ────

#[tokio::test]
async fn approve_and_remember_creates_policy_atomically() {
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let policies: Arc<dyn HotlPolicyStore> = Arc::new(InMemoryHotlPolicyStore::new());
    let app = router(build_state(StateOptions {
        decision_store: Some(Arc::clone(&decisions)),
        policy_store: Some(Arc::clone(&policies)),
        ..Default::default()
    }));
    let body = serde_json::json!({
        "request_id": Uuid::new_v4().to_string(),
        "verdict": "allow",
        "decided_by": "alice",
        "raise_policy": {
            "scope": "llm_call",
            "window_seconds": 3600,
            "max_count": 10
        }
    });
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
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = body_json(resp.into_body()).await;
    let policy = &json["policy_created"];
    assert!(
        policy.is_object(),
        "policy_created must be present when raise_policy is set"
    );
    assert_eq!(policy["scope"], "llm_call");
    assert_eq!(policy["window_seconds"], 3600);
    assert_eq!(policy["max_count"], 10);

    // The follow-up create must also be visible via the policy store.
    let tenant_id = Uuid::nil(); // auth: None → handler uses nil UUID
    let listed = policies.list(tenant_id, Some("llm_call")).await.unwrap();
    assert_eq!(listed.len(), 1, "policy must be persisted in the store");
}

// ── 5. raise_policy with no limits → 400 (documented gap-fill for plan #5) ──
//
// The plan (§4 test plan, case #5) flagged this case as awkward in 3a.1 because
// no `hotl_escalations` parent table exists, so a request_id can't be "unknown"
// in a meaningful sense. We reinterpret #5 as a route-layer validation guard:
// a `raise_policy` with neither `max_count` nor `max_usd` returns 400 because
// the policy store would reject it anyway, and surfacing it earlier as
// `invalid_request` gives the chat-ui a stable error code to switch on.
#[tokio::test]
async fn raise_policy_with_no_limits_returns_400() {
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let policies: Arc<dyn HotlPolicyStore> = Arc::new(InMemoryHotlPolicyStore::new());
    let app = router(build_state(StateOptions {
        decision_store: Some(Arc::clone(&decisions)),
        policy_store: Some(Arc::clone(&policies)),
        ..Default::default()
    }));
    let body = serde_json::json!({
        "request_id": Uuid::new_v4().to_string(),
        "verdict": "allow",
        "decided_by": "alice",
        "raise_policy": {
            "scope": "llm_call",
            "window_seconds": 3600
            // max_count and max_usd both omitted
        }
    });
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
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ── 6. Duplicate request_id returns 409 ──────────────────────────────────────

#[tokio::test]
async fn duplicate_request_id_returns_409() {
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let app = router(build_state(StateOptions {
        decision_store: Some(Arc::clone(&decisions)),
        ..Default::default()
    }));
    let request_id = Uuid::new_v4();
    let body = serde_json::json!({
        "request_id": request_id.to_string(),
        "verdict": "allow",
        "decided_by": "alice"
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();

    // First POST: 201.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/hotl/decisions")
                .header("content-type", "application/json")
                .body(Body::from(body_bytes.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Second POST with the same request_id: 409.
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/hotl/decisions")
                .header("content-type", "application/json")
                .body(Body::from(body_bytes))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

// ── 7. Unauthorized when bearer missing ──────────────────────────────────────

#[tokio::test]
async fn unauthorized_when_bearer_missing() {
    let validator: Arc<dyn TokenValidator> = Arc::new(StubValidator {
        claims: Claims {
            sub: "u".into(),
            tenant_id: "00000000-0000-0000-0000-000000000abc".into(),
            roles: vec!["system_admin".into()],
            scopes: vec![],
        },
    });
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let app = router(build_state(StateOptions {
        decision_store: Some(decisions),
        auth: Some(validator),
        ..Default::default()
    }));
    let body = serde_json::json!({
        "request_id": Uuid::new_v4().to_string(),
        "verdict": "allow",
        "decided_by": "alice"
    });
    // No Authorization header.
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
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ── 8. Forbidden when role has no policy match ──────────────────────────────
//
// The current Casbin policy file has no `/hotl/*` rules — only
// `system_admin, *, *, *` matches by default. A role like `nobody` has no
// matching rule, so the middleware returns 403. This documents the
// expected behaviour for 3a.1 (no dedicated `hotl:decide` scope yet);
// once the policy file gains a `/hotl/decisions, write` rule for
// `tenant_admin`, the test will need to use a more restricted role.
#[tokio::test]
async fn forbidden_when_rbac_denies_hotl_decide_scope() {
    let validator: Arc<dyn TokenValidator> = Arc::new(StubValidator {
        claims: Claims {
            sub: "u".into(),
            tenant_id: "00000000-0000-0000-0000-000000000abc".into(),
            roles: vec!["nobody".into()],
            scopes: vec![],
        },
    });
    let authz = Arc::new(Authz::new_default().await.expect("authz"));
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let app = router(build_state(StateOptions {
        decision_store: Some(decisions),
        auth: Some(validator),
        authz: Some(authz),
        ..Default::default()
    }));
    let body = serde_json::json!({
        "request_id": Uuid::new_v4().to_string(),
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
}

// ── 9. `escalation_id` alias parses ──────────────────────────────────────────

#[tokio::test]
async fn escalation_id_alias_parses() {
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let app = router(build_state(StateOptions {
        decision_store: Some(Arc::clone(&decisions)),
        ..Default::default()
    }));
    let escalation_id = Uuid::new_v4();
    let body = serde_json::json!({
        // Use the wire field name from the SSE event — the route accepts
        // it as a serde alias for request_id so the existing frontend +
        // e2e mocks keep working.
        "escalation_id": escalation_id.to_string(),
        "verdict": "allow",
        "decided_by": "alice"
    });
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
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["request_id"], escalation_id.to_string());
}

// ── 10. Audit sink receives the entry (defence-in-depth coverage) ────────────

#[tokio::test]
async fn audit_sink_receives_decision_entry() {
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let sink_obj = Arc::new(InMemoryHotlAuditSink::new());
    let sink: Arc<dyn HotlAuditSink> = sink_obj.clone();
    let app = router(build_state(StateOptions {
        decision_store: Some(decisions),
        audit_sink: Some(sink),
        ..Default::default()
    }));
    let body = serde_json::json!({
        "request_id": Uuid::new_v4().to_string(),
        "verdict": "deny",
        "decided_by": "alice"
    });
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
    assert_eq!(resp.status(), StatusCode::CREATED);

    let entries = sink_obj.snapshot();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].action, "hotl.decision");
    assert_eq!(entries[0].actor, "alice");
}

// ── 11. S12-6: live waiter is resolved → resumed:true ────────────────────────
//
// Pre-register a ticket on the shared `DecisionRegistry`, then POST
// `/v1/hotl/decisions` for the same request_id. The response MUST carry
// `resumed: true` AND the awaiting ticket must resolve with the operator's
// verdict before a bounded timeout.
#[tokio::test]
async fn decision_resolves_live_waiter_returns_resumed_true() {
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let registry = Arc::new(DecisionRegistry::new());
    let request_id = Uuid::new_v4();
    // Park a ticket so the route handler has someone to wake.
    let ticket = registry.register(request_id, Instant::now() + Duration::from_secs(60));

    let app = router(build_state(StateOptions {
        decision_store: Some(Arc::clone(&decisions)),
        decision_registry: Some(Arc::clone(&registry)),
        ..Default::default()
    }));
    let body = serde_json::json!({
        "request_id": request_id.to_string(),
        "verdict": "allow",
        "decided_by": "alice"
    });
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
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = body_json(resp.into_body()).await;
    assert_eq!(
        json["resumed"], true,
        "live waiter must flip resumed to true"
    );

    // Ticket must resolve to the operator's verdict (Allow, decided_by alice).
    let cancel = CancellationToken::new();
    let settled = tokio::time::timeout(Duration::from_secs(2), ticket.await_decision(&cancel))
        .await
        .expect("ticket must resolve before the bounded timeout")
        .expect("ticket must not error out");
    assert_eq!(settled.verdict, HotlResolution::Allow);
    assert_eq!(settled.decided_by.as_deref(), Some("alice"));
}

// ── 12. S12-6: no waiter present → resumed:false ─────────────────────────────
//
// Locks in the sprint-11 behaviour: when no ticket was registered, the
// route handler still returns 201 and `resumed: false`. This is the
// dominant case for the legacy `EnforcerGate` path that never suspends.
#[tokio::test]
async fn decision_with_no_waiter_returns_resumed_false() {
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let registry = Arc::new(DecisionRegistry::new());
    let app = router(build_state(StateOptions {
        decision_store: Some(Arc::clone(&decisions)),
        decision_registry: Some(Arc::clone(&registry)),
        ..Default::default()
    }));
    let body = serde_json::json!({
        "request_id": Uuid::new_v4().to_string(),
        "verdict": "allow",
        "decided_by": "alice"
    });
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
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = body_json(resp.into_body()).await;
    assert_eq!(
        json["resumed"], false,
        "no live waiter ⇒ resumed must remain false"
    );
}

// ── 13. S12-6: late decision after ticket timed out → resumed:false ──────────
//
// Register a ticket with a 50ms expiry, let the background sleeper fire
// `resolve(.., Timeout)` (which removes the entry from the map), then
// POST `/v1/hotl/decisions` for that same request_id. The decision row
// is fresh from the store's perspective (no duplicate), so the response is
// 201 + `resumed: false` (no live waiter when the late decision arrived).
#[tokio::test]
async fn late_decision_after_timeout_returns_resumed_false() {
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let registry = Arc::new(DecisionRegistry::new());
    let request_id = Uuid::new_v4();

    // Short-lived ticket; immediately await it so we observe the timeout.
    let ticket = registry.register(request_id, Instant::now() + Duration::from_millis(50));
    let cancel = CancellationToken::new();
    let settled = ticket
        .await_decision(&cancel)
        .await
        .expect("ticket must resolve via timeout, not error");
    assert_eq!(
        settled.verdict,
        HotlResolution::Timeout,
        "ticket must settle as Timeout when expiry fires before any resolve"
    );

    // Give the spawned background timeout task a beat to clear the slot.
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        registry.is_empty(),
        "background timeout must have removed the slot"
    );

    let app = router(build_state(StateOptions {
        decision_store: Some(Arc::clone(&decisions)),
        decision_registry: Some(Arc::clone(&registry)),
        ..Default::default()
    }));
    let body = serde_json::json!({
        "request_id": request_id.to_string(),
        "verdict": "allow",
        "decided_by": "alice"
    });
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
    // Decision row is fresh in the store — 201, not 409. Late decision finds
    // no live waiter — resumed stays false.
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = body_json(resp.into_body()).await;
    assert_eq!(
        json["resumed"], false,
        "late decision after timeout ⇒ resumed must be false"
    );

    // Defensive: HotlTicketError is only reached via ChannelDropped, which
    // these tests never exercise — silence the unused-import lint cleanly.
    let _ = std::any::type_name::<HotlTicketError>();
}
