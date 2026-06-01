//! Sprint-13 S13-11 — cross-feature `HotL` hardening regression bundle (part 2).
//!
//! Part 1 lives in `crates/xiaoguai-core/tests/hotl_cross_feature_e2e.rs`
//! and pins the gate-side suspend semantics.
//!
//! This file pins the **restart-replay + scope-gate combo** end-to-end
//! through the `POST /v1/hotl/decisions` route:
//!
//! * S13-5  — `DecisionRegistry::replay_from_storage` reattaches all
//!            still-pending unexpired rows on boot.
//! * S13-8  — the route accepts `escalation_id` (no `request_id` alias).
//! * S13-10 — Casbin `hotl:decide` scope is required; missing scope ⇒ 403,
//!            present scope + matching live waiter ⇒ 201 with
//!            `resumed: true` and the suspended ticket settles with the
//!            operator's verdict.
//!
//! Bundle scope per the sprint-13 plan §1.1 — this test aggregates
//! already-passing surfaces. RED was decorative; the regression bundle
//! consolidates per-feature surfaces into a multi-axis e2e scenario.
//!
//! Cross-refs: lld-agent.md §4.6, CASE-HOTL-005..013 (test-spec.md).
//!
//! No PG required — uses the same in-memory mock store pattern as
//! S13-5's `hotl_persistence_replay.rs`. Live PG coverage of the
//! underlying `HotlEscalationStore` lives in
//! `xiaoguai-storage/tests/hotl_escalations_repo.rs`
//! (`#[ignore = "requires Docker"]`).

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::auth::{Claims, StubValidator, TokenValidator};
use xiaoguai_api::hotl::audit::{HotlAuditSink, InMemoryHotlAuditSink};
use xiaoguai_api::hotl::decision::{HotlDecisionStore, InMemoryHotlDecisionStore};
use xiaoguai_api::hotl::decision_registry::DecisionRegistry;
use xiaoguai_api::{router, AppState, CancelRegistry};
use xiaoguai_auth::{Authz, DbPolicyRow};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend};
use xiaoguai_storage::repositories::error::RepoResult;
use xiaoguai_storage::repositories::hotl_escalations::{
    HotlDecisionVerdict as StoreVerdict, HotlEscalationRow, HotlEscalationStore, HotlPendingRow,
};

use common::{InMemoryMessageRepo, InMemorySessionRepo};

// ── mock store ────────────────────────────────────────────────────────────────

/// In-memory `HotlEscalationStore` — same shape as
/// `MockHotlEscalationStore` in `hotl_persistence_replay.rs`, duplicated
/// here so the cross-feature test stays self-contained. Pre-seeded with
/// fixture rows that simulate a pre-restart PG state.
#[derive(Debug, Default)]
struct MockEscalationStore {
    parents: Mutex<HashMap<Uuid, HotlEscalationRow>>,
    children: Mutex<HashMap<Uuid, HotlPendingRow>>,
}

impl MockEscalationStore {
    fn arc() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Pre-seed both tables — simulates state surviving a restart.
    fn seed_pending(&self, parent: HotlEscalationRow, mut child: HotlPendingRow) {
        child.escalation_id = parent.id;
        self.parents.lock().insert(parent.id, parent.clone());
        self.children.lock().insert(parent.id, child);
    }

    fn child_status(&self, escalation_id: Uuid) -> Option<String> {
        self.children
            .lock()
            .get(&escalation_id)
            .map(|r| r.status.clone())
    }
}

#[async_trait]
impl HotlEscalationStore for MockEscalationStore {
    async fn insert_pending(
        &self,
        parent: HotlEscalationRow,
        child: HotlPendingRow,
    ) -> RepoResult<Uuid> {
        let pinned = HotlPendingRow {
            escalation_id: parent.id,
            ..child
        };
        self.parents.lock().insert(parent.id, parent.clone());
        self.children.lock().insert(parent.id, pinned);
        Ok(parent.id)
    }

    async fn list_pending_unexpired(&self, now: DateTime<Utc>) -> RepoResult<Vec<HotlPendingRow>> {
        let mut out: Vec<HotlPendingRow> = self
            .children
            .lock()
            .values()
            .filter(|r| r.status == "pending" && r.expires_at > now)
            .cloned()
            .collect();
        out.sort_by_key(|r| r.created_at);
        Ok(out)
    }

    async fn record_decision(
        &self,
        escalation_id: Uuid,
        verdict: StoreVerdict,
        decided_by: Option<String>,
    ) -> RepoResult<bool> {
        let mut guard = self.children.lock();
        let Some(entry) = guard.get_mut(&escalation_id) else {
            return Ok(false);
        };
        if entry.status != "pending" {
            return Ok(false);
        }
        entry.status = verdict.status_str().to_string();
        entry.decided_at = Some(Utc::now());
        entry.decided_by = decided_by;
        Ok(true)
    }
}

// ── fixtures ──────────────────────────────────────────────────────────────────

fn parent_row(id: Uuid, scope: &str) -> HotlEscalationRow {
    HotlEscalationRow {
        id,
        tenant_id: Uuid::new_v4(),
        session_id: Uuid::new_v4(),
        top_level_scope: scope.to_string(),
        status: "pending".to_string(),
        created_at: Utc::now(),
        parent_id: None,
    }
}

fn child_row(escalation_id: Uuid, scope: &str, expires_at: DateTime<Utc>) -> HotlPendingRow {
    HotlPendingRow {
        id: Uuid::new_v4(),
        escalation_id,
        tenant_id: Uuid::new_v4(),
        scope: scope.to_string(),
        tool: scope.to_string(),
        args_redacted: serde_json::json!({}),
        status: "pending".to_string(),
        expires_at,
        created_at: Utc::now(),
        decided_at: None,
        decided_by: None,
    }
}

/// Migration 0027 row that S13-10 expects to be merged into the Casbin
/// enforcer. Mirrors `hotl_decide_scope.rs::seeded_hotl_decide_row`.
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
        // `system_admin` satisfies the existing path-based RBAC layer;
        // the scope check is layered on top.
        roles: vec!["system_admin".into()],
        scopes: scopes.into_iter().map(String::from).collect(),
    }
}

fn build_state(
    decisions: Arc<dyn HotlDecisionStore>,
    audit: Arc<dyn HotlAuditSink>,
    auth: Arc<dyn TokenValidator>,
    authz: Arc<Authz>,
    registry: Arc<DecisionRegistry>,
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
        hotl_audit: Some(audit),
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
        decision_registry: registry,
    }
}

async fn body_json(body: Body) -> Value {
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

// ── test 2: restart-replay + scope-gate combo ────────────────────────────────

/// End-to-end restart + scope-gate scenario:
///
/// 1. Pre-seed 2 `hotl_pending` rows on the mock store (simulating a
///    pre-restart PG snapshot).
/// 2. `DecisionRegistry::replay_from_storage` rebuilds the in-memory
///    waiter map (S13-5) — both rows must surface as reattached.
/// 3. `POST /v1/hotl/decisions` for one of the ids using an operator JWT
///    WITHOUT `hotl:decide` scope → 403 + nested envelope
///    `error.details.scope = "hotl:decide"` (sprint-14 S14-1)
///    body (S13-10).
/// 4. `POST` the same id with `hotl:decide` scope → 201, body uses
///    `escalation_id` (S13-8), `resumed=true` (S13-5 waiter resolved),
///    audit sink captured the decision row (sprint-11 contract).
/// 5. The pending row's status in the store is now `resolved` — the
///    DB write happened BEFORE the in-memory oneshot fired (S13-5
///    persist-first ordering).
#[tokio::test]
async fn restart_replay_then_resolve_via_route_with_scope_check() {
    // ── pre-restart fixture: 2 pending rows on the "previous" PG. ──────
    let store = MockEscalationStore::arc();
    let future = Utc::now() + chrono::Duration::hours(1);

    let id_a = Uuid::new_v4();
    store.seed_pending(
        parent_row(id_a, "tool_call.execute_python"),
        child_row(id_a, "tool_call.execute_python", future),
    );

    let id_b = Uuid::new_v4();
    store.seed_pending(
        parent_row(id_b, "mcp.oauth.consent"),
        child_row(id_b, "mcp.oauth.consent", future),
    );

    // ── boot: replay rebuilds the registry. ────────────────────────────
    let registry = DecisionRegistry::replay_from_storage(
        store.clone() as Arc<dyn HotlEscalationStore>,
        Utc::now(),
    )
    .await
    .expect("S13-5: replay_from_storage must succeed");

    assert_eq!(
        registry.len(),
        2,
        "S13-5: both pre-seeded pending rows must be reattached as live waiters"
    );

    // ── boot: authz merges the seeded `hotl:decide` rule. ──────────────
    let mut authz = Authz::new_default().await.expect("authz");
    authz
        .merge_db_policies(vec![seeded_hotl_decide_row()])
        .await
        .expect("S13-10: merge_db_policies for hotl:decide");
    let authz = Arc::new(authz);

    let decisions: Arc<dyn HotlDecisionStore> = Arc::new(InMemoryHotlDecisionStore::new());
    let audit_sink_obj = Arc::new(InMemoryHotlAuditSink::new());
    let audit: Arc<dyn HotlAuditSink> = audit_sink_obj.clone();

    // ── 3. operator WITHOUT hotl:decide scope → 403. ───────────────────
    let validator_noscope: Arc<dyn TokenValidator> = Arc::new(StubValidator {
        claims: claims_with(vec!["read:audit"]),
    });
    let app_noscope = router(build_state(
        decisions.clone(),
        audit.clone(),
        validator_noscope,
        authz.clone(),
        registry.clone(),
    ));
    let body_noscope = serde_json::json!({
        "escalation_id": id_a.to_string(),
        "verdict": "allow",
        "decided_by": "alice"
    });
    let resp = app_noscope
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/hotl/decisions")
                .header(header::AUTHORIZATION, "Bearer t")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body_noscope).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "S13-10: JWT without hotl:decide must be rejected"
    );
    let json = body_json(resp.into_body()).await;
    // Sprint-14 S14-1: api-contract §1.6 nested envelope (was flat
    // `required_scope` field in sprint-13).
    assert_eq!(json["error"]["code"], "scope_required", "json: {json}");
    assert_eq!(
        json["error"]["details"]["scope"], "hotl:decide",
        "json: {json}"
    );
    assert_eq!(
        store.child_status(id_a).as_deref(),
        Some("pending"),
        "S13-10: scope rejection must NOT mutate the pending row"
    );
    assert_eq!(
        registry.len(),
        2,
        "S13-10: scope rejection must NOT consume the live waiter"
    );

    // ── 4. operator WITH hotl:decide scope → 201, resumed=true. ────────
    let validator_scoped: Arc<dyn TokenValidator> = Arc::new(StubValidator {
        claims: claims_with(vec!["hotl:decide"]),
    });
    let app_scoped = router(build_state(
        decisions,
        audit,
        validator_scoped,
        authz,
        registry.clone(),
    ));
    let body_scoped = serde_json::json!({
        "escalation_id": id_a.to_string(),
        "verdict": "allow",
        "decided_by": "alice"
    });
    let resp = app_scoped
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/hotl/decisions")
                .header(header::AUTHORIZATION, "Bearer t")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body_scoped).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "S13-10: JWT with hotl:decide must be accepted"
    );
    let json = body_json(resp.into_body()).await;
    assert_eq!(
        json["escalation_id"],
        id_a.to_string(),
        "S13-8: response body must echo escalation_id (no request_id alias)"
    );
    assert!(
        json.get("request_id").is_none(),
        "S13-8: response body must NOT include legacy request_id"
    );
    assert_eq!(
        json["resumed"], true,
        "S13-5: live waiter from replay must be resolved by the route handler"
    );

    // ── 5. persist-first ordering: store row is `resolved` BEFORE the
    //      in-memory waiter went away. ────────────────────────────────
    assert_eq!(
        store.child_status(id_a).as_deref(),
        Some("resolved"),
        "S13-5: hotl_pending row must be UPDATEd to resolved"
    );

    // The resolved waiter must be gone; the OTHER one (id_b) is still
    // parked.
    assert_eq!(
        registry.len(),
        1,
        "S13-5: resolved waiter removed, the other replay survivor remains"
    );

    // Sprint-11 audit chain: exactly one hotl.decision audit row.
    let audit_entries = audit_sink_obj.snapshot();
    assert_eq!(
        audit_entries.len(),
        1,
        "audit sink must record exactly one hotl.decision entry; got {audit_entries:?}",
    );
    assert_eq!(audit_entries[0].action, "hotl.decision");
    assert_eq!(audit_entries[0].actor, "alice");

    // Give any background sleep_until companion a beat — sanity guard
    // that the test completed well under the brief's 60s warm budget.
    tokio::time::sleep(Duration::from_millis(10)).await;
}
