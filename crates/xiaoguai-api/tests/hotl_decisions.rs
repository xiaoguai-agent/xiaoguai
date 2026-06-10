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
use axum::http::{Request, StatusCode};
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
        teams: None,
        incidents: None,
        team_audit: None,
        watchers: None,
        loops: None,
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
        "escalation_id": Uuid::new_v4().to_string(),
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
        "escalation_id": Uuid::new_v4().to_string(),
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
        "escalation_id": Uuid::new_v4().to_string(),
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
    let listed = policies.list(Some("llm_call")).await.unwrap();
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
        "escalation_id": Uuid::new_v4().to_string(),
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

// ── 6. Duplicate escalation_id returns 409 ───────────────────────────────────

#[tokio::test]
async fn duplicate_escalation_id_returns_409() {
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let app = router(build_state(StateOptions {
        decision_store: Some(Arc::clone(&decisions)),
        ..Default::default()
    }));
    let escalation_id = Uuid::new_v4();
    let body = serde_json::json!({
        "escalation_id": escalation_id.to_string(),
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

    // Second POST with the same escalation_id: 409.
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
        claims: Claims { sub: "u".into() },
    });
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let app = router(build_state(StateOptions {
        decision_store: Some(decisions),
        auth: Some(validator),
        ..Default::default()
    }));
    let body = serde_json::json!({
        "escalation_id": Uuid::new_v4().to_string(),
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

// ── 8. `escalation_id` is the canonical field — body echoes it back ──────────
//
// Sprint-13 S13-8 / DEC-HLD-016: the field renamed from `request_id` to
// `escalation_id`. The response body must mirror the canonical name; the
// legacy `request_id` key must NOT appear. Bodies that POST the legacy
// name are rejected with a structured 400 (covered separately in
// `tests/hotl_escalation_id_rename.rs`).
#[tokio::test]
async fn response_body_uses_canonical_escalation_id() {
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let app = router(build_state(StateOptions {
        decision_store: Some(Arc::clone(&decisions)),
        ..Default::default()
    }));
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
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = body_json(resp.into_body()).await;
    assert_eq!(json["escalation_id"], escalation_id.to_string());
    assert!(
        json.get("request_id").is_none(),
        "response body must NOT include the legacy `request_id` field after S13-8: {json}"
    );
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
        "escalation_id": Uuid::new_v4().to_string(),
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
// `/v1/hotl/decisions` for the same escalation_id. The response MUST carry
// `resumed: true` AND the awaiting ticket must resolve with the operator's
// verdict before a bounded timeout.
#[tokio::test]
async fn decision_resolves_live_waiter_returns_resumed_true() {
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let registry = Arc::new(DecisionRegistry::new());
    let escalation_id = Uuid::new_v4();
    // Park a ticket so the route handler has someone to wake.
    let ticket = registry.register(escalation_id, Instant::now() + Duration::from_secs(60));

    let app = router(build_state(StateOptions {
        decision_store: Some(Arc::clone(&decisions)),
        decision_registry: Some(Arc::clone(&registry)),
        ..Default::default()
    }));
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
        "escalation_id": Uuid::new_v4().to_string(),
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
// POST `/v1/hotl/decisions` for that same escalation_id. The decision row
// is fresh from the store's perspective (no duplicate), so the response is
// 201 + `resumed: false` (no live waiter when the late decision arrived).
#[tokio::test]
async fn late_decision_after_timeout_returns_resumed_false() {
    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let registry = Arc::new(DecisionRegistry::new());
    let escalation_id = Uuid::new_v4();

    // Short-lived ticket; immediately await it so we observe the timeout.
    let ticket = registry.register(escalation_id, Instant::now() + Duration::from_millis(50));
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

// ── 14-16. Pre-flight escalation existence check (audit F1b) ─────────────────
//
// When the registry's store supports `lookup` (sqlite-backed production
// deployments), `POST /v1/hotl/decisions` validates the escalation row
// BEFORE recording the decision: unknown id → 404 with NO phantom
// decision row; terminal row → 409. Stores without lookup support
// (`NoopHotlEscalationStore` — tests 2/12/13 above) return `Unsupported`
// and keep the legacy always-201 contract.

mod preflight {
    use super::*;
    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use xiaoguai_api::hotl::decision::InMemoryHotlDecisionStore as ConcreteDecisionStore;
    use xiaoguai_api::hotl::decision_registry::EscalationLookup;
    use xiaoguai_storage::repositories::error::RepoResult;
    use xiaoguai_storage::repositories::hotl_escalations::{
        HotlDecisionVerdict as StoreVerdict, HotlEscalationRow, HotlEscalationStore, HotlPendingRow,
    };

    /// Store stub whose `lookup` always returns the configured answer.
    /// `record_decision` reports a match so the resolve path stays on the
    /// happy branch when the pre-flight allows the request through.
    #[derive(Debug)]
    struct FixedLookupStore(EscalationLookup);

    #[async_trait]
    impl HotlEscalationStore for FixedLookupStore {
        async fn insert_pending(
            &self,
            parent: HotlEscalationRow,
            _child: HotlPendingRow,
        ) -> RepoResult<Uuid> {
            Ok(parent.id)
        }

        async fn list_pending_unexpired(
            &self,
            _now: DateTime<Utc>,
        ) -> RepoResult<Vec<HotlPendingRow>> {
            Ok(Vec::new())
        }

        async fn record_decision(
            &self,
            _escalation_id: Uuid,
            _verdict: StoreVerdict,
            _decided_by: Option<String>,
        ) -> RepoResult<bool> {
            Ok(true)
        }

        async fn lookup(&self, _escalation_id: Uuid) -> RepoResult<EscalationLookup> {
            Ok(self.0.clone())
        }
    }

    fn app_with_lookup(lookup: EscalationLookup) -> (axum::Router, Arc<ConcreteDecisionStore>) {
        let decisions = Arc::new(ConcreteDecisionStore::new());
        let registry = Arc::new(DecisionRegistry::with_store(Arc::new(FixedLookupStore(
            lookup,
        ))));
        let app = router(build_state(StateOptions {
            decision_store: Some(decisions.clone() as Arc<dyn HotlDecisionStore>),
            decision_registry: Some(registry),
            ..Default::default()
        }));
        (app, decisions)
    }

    async fn post_decision(app: axum::Router, escalation_id: Uuid) -> axum::response::Response {
        let body = serde_json::json!({
            "escalation_id": escalation_id.to_string(),
            "verdict": "allow",
            "decided_by": "alice"
        });
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/hotl/decisions")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn unknown_escalation_returns_404_with_no_phantom_decision_row() {
        let (app, decisions) = app_with_lookup(EscalationLookup::NotFound);
        let escalation_id = Uuid::new_v4();
        let resp = post_decision(app, escalation_id).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let json = body_json(resp.into_body()).await;
        assert_eq!(json["code"], "not_found");
        assert!(
            json["message"]
                .as_str()
                .unwrap()
                .contains(&escalation_id.to_string()),
            "404 must name the offending escalation_id: {json}"
        );
        assert!(
            decisions.snapshot().is_empty(),
            "the pre-flight 404 must NOT leave a phantom decision row behind"
        );
    }

    #[tokio::test]
    async fn terminal_escalation_returns_409_with_no_decision_row() {
        let (app, decisions) = app_with_lookup(EscalationLookup::Terminal {
            status: "expired".to_string(),
            at: Utc::now(),
        });
        let escalation_id = Uuid::new_v4();
        let resp = post_decision(app, escalation_id).await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let json = body_json(resp.into_body()).await;
        assert_eq!(json["code"], "conflict");
        assert!(
            json["message"].as_str().unwrap().contains("expired"),
            "409 must name the terminal status: {json}"
        );
        assert!(
            decisions.snapshot().is_empty(),
            "a terminal escalation must NOT acquire a new decision row"
        );
    }

    #[tokio::test]
    async fn pending_escalation_proceeds_to_201() {
        let (app, decisions) = app_with_lookup(EscalationLookup::Pending);
        let resp = post_decision(app, Uuid::new_v4()).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let json = body_json(resp.into_body()).await;
        // No live in-memory waiter was registered — resumed stays false.
        assert_eq!(json["resumed"], false);
        assert_eq!(
            decisions.snapshot().len(),
            1,
            "decision row must be recorded"
        );
    }
}

// ── GET /v1/hotl/pending — parked-tick visibility (LLD-LOOP-001 §7) ──────────

async fn get_pending(app: axum::Router) -> (StatusCode, Value) {
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/hotl/pending")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

#[tokio::test]
async fn pending_empty_when_store_has_none() {
    // Default registry uses the no-op escalation store → empty list, 200.
    let app = router(build_state(StateOptions::default()));
    let (status, body) = get_pending(app).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!([]));
}

#[tokio::test]
async fn pending_lists_parked_escalations_with_session() {
    use xiaoguai_storage::repositories::hotl_escalations::{
        HotlEscalationRow, HotlPendingRow, SqliteHotlEscalationRepository,
    };

    // Real sqlite escalation store with one live pending row.
    let dir = tempfile::tempdir().unwrap();
    let pool = xiaoguai_storage::db::connect(dir.path().join("t.db").to_str().unwrap(), 5)
        .await
        .unwrap();
    xiaoguai_storage::db::migrate(&pool).await.unwrap();
    let store = SqliteHotlEscalationRepository::new(pool);

    let session_id = Uuid::new_v4();
    let parent = HotlEscalationRow {
        id: Uuid::new_v4(),
        session_id,
        top_level_scope: "tool_call.execute_python".into(),
        status: "pending".into(),
        created_at: chrono::Utc::now(),
        parent_id: None,
    };
    let child = HotlPendingRow {
        id: Uuid::new_v4(),
        escalation_id: Uuid::nil(),
        scope: "tool_call.execute_python".into(),
        tool: "execute_python".into(),
        args_redacted: serde_json::json!({"code": "<redacted>"}),
        status: "pending".into(),
        expires_at: chrono::Utc::now() + chrono::Duration::hours(24),
        created_at: chrono::Utc::now(),
        decided_at: None,
        decided_by: None,
    };
    let escalation_id =
        xiaoguai_storage::repositories::HotlEscalationStore::insert_pending(&store, parent, child)
            .await
            .unwrap();

    let registry = Arc::new(DecisionRegistry::with_store(Arc::new(store)));
    let app = router(build_state(StateOptions {
        decision_registry: Some(registry),
        ..Default::default()
    }));

    let (status, body) = get_pending(app).await;
    assert_eq!(status, StatusCode::OK);
    let arr = body.as_array().expect("array");
    assert_eq!(arr.len(), 1, "the parked escalation must be visible");
    assert_eq!(arr[0]["escalation_id"], escalation_id.to_string());
    assert_eq!(
        arr[0]["session_id"],
        session_id.to_string(),
        "operator must see which session/loop is parked"
    );
    assert_eq!(arr[0]["tool"], "execute_python");
}
