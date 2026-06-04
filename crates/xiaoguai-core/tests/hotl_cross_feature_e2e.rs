//! Sprint-13 S13-11 — cross-feature `HotL` hardening regression bundle (part 1).
//!
//! This file pins the **integration smoke** that exercises every sprint-13
//! axis in one go through the `SuspendingHotlGate`:
//!
//! * S13-4 / S13-6 — args redaction (`$.password` masked to `"***"`).
//! * S13-5         — `DecisionRegistry` persistence (parent + child rows).
//! * S13-6         — `redaction_policy_id` threaded into the audit row's
//!                   `details` JSON.
//! * S13-7         — per-scope-class expiry lookup (`tool` → 6h).
//! * S13-8         — `HotlGateVerdict::Suspend` carries `escalation_id`
//!                   (not `request_id`).
//!
//! Companion file: `crates/xiaoguai-api/tests/hotl_cross_feature_e2e.rs`
//! pins the route-shaped restart-replay + scope-gate scenario.
//!
//! Bundle scope per the sprint-13 plan §1.1 — these tests aggregate
//! already-passing surfaces; no behaviour is added. The RED commit was
//! decorative.
//!
//! Cross-refs: lld-agent.md §4.6, CASE-HOTL-005..013 (test-spec.md).
//!
//! No PG required — the in-memory mock store mirrors the
//! `PgHotlEscalationRepository` contract row-for-row. The matching
//! integration coverage against the live PG schema lives in
//! `xiaoguai-storage/tests/hotl_escalations_repo.rs`
//! (`#[ignore = "requires Docker"]`).

#![cfg(test)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde_json::json;
use uuid::Uuid;

use xiaoguai_api::hotl::audit::HotlAuditSink;
use xiaoguai_api::hotl::decision_registry::DecisionRegistry;
use xiaoguai_api::hotl::enforcer::{HotlEnforcer, HotlVerdict, HotlVerdictResult};
use xiaoguai_audit::AuditEntry;
use xiaoguai_core::hotl_bridge::SuspendingHotlGate;
use xiaoguai_storage::repositories::error::{RepoError, RepoResult};
use xiaoguai_storage::repositories::hotl_escalations::{
    HotlDecisionVerdict as StoreVerdict, HotlEscalationRow, HotlEscalationStore, HotlPendingRow,
};
use xiaoguai_storage::repositories::hotl_redaction::{HotlRedactionRepo, RedactionPolicyRow};

// ── mock stores ───────────────────────────────────────────────────────────────

/// In-memory `HotlEscalationStore`. Mirrors S13-5's
/// `tests/hotl_persistence_replay.rs::MockHotlEscalationStore` but is
/// duplicated here to keep this test self-contained (no `mod common`).
#[derive(Debug, Default)]
struct MockEscalationStore {
    parents: Mutex<HashMap<Uuid, HotlEscalationRow>>,
    children: Mutex<HashMap<Uuid, HotlPendingRow>>,
}

impl MockEscalationStore {
    fn arc() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn parent(&self, id: Uuid) -> Option<HotlEscalationRow> {
        self.parents.lock().get(&id).cloned()
    }

    fn child(&self, escalation_id: Uuid) -> Option<HotlPendingRow> {
        self.children.lock().get(&escalation_id).cloned()
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

/// In-memory `HotlRedactionRepo`. Mirrors S13-6's
/// `StubRedactionRepo` but is duplicated here for self-containment.
#[derive(Debug, Clone)]
struct StubRedactionRepo {
    rows: Vec<RedactionPolicyRow>,
    fail: bool,
}

impl StubRedactionRepo {
    fn with_rule(row: RedactionPolicyRow) -> Arc<Self> {
        Arc::new(Self {
            rows: vec![row],
            fail: false,
        })
    }
}

#[async_trait]
impl HotlRedactionRepo for StubRedactionRepo {
    async fn load_all(&self) -> RepoResult<Vec<RedactionPolicyRow>> {
        if self.fail {
            return Err(RepoError::InvalidArgument("forced failure".into()));
        }
        Ok(self.rows.clone())
    }
}

/// Always-escalate enforcer so the gate exercises the suspend branch.
#[derive(Debug)]
struct AlwaysEscalate;

#[async_trait]
impl HotlEnforcer for AlwaysEscalate {
    async fn check(&self, _scope: &str, _amount: f64) -> HotlVerdictResult {
        Ok(HotlVerdict::Escalate("test escalate".into()))
    }
}

/// Capturing audit sink — records appended entries so the test can
/// assert on the `details` JSON.
#[derive(Debug, Default)]
struct CaptureAuditSink {
    entries: parking_lot::Mutex<Vec<AuditEntry>>,
}

impl CaptureAuditSink {
    fn arc() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn entries(&self) -> Vec<AuditEntry> {
        self.entries.lock().clone()
    }
}

#[async_trait]
impl HotlAuditSink for CaptureAuditSink {
    async fn append(&self, entry: AuditEntry) -> Result<(), String> {
        self.entries.lock().push(entry);
        Ok(())
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn rule(scope: &str, jsonpath: &str) -> RedactionPolicyRow {
    RedactionPolicyRow {
        id: Uuid::new_v4(),
        scope: scope.into(),
        jsonpath: jsonpath.into(),
        applies_to: vec!["sse".into()],
        created_at: Utc::now(),
    }
}

// ── test 1: full cross-feature smoke ──────────────────────────────────────────

/// Drive one escalation that exercises every sprint-13 axis at once:
///
/// 1. Tenant has a redaction policy `{scope: "tool_call.execute_python",
///    jsonpath: "$.password", applies_to: ["sse"]}` (S13-3, S13-4).
/// 2. `agent.hotl.expiry.tool = 6h` per-scope override (S13-7).
/// 3. Escalation for scope `tool_call.execute_python` with args
///    `{password: "x", code: "print('y')"}`.
///
/// Asserts:
///
/// * (a) `Suspend` verdict carries `args_redacted = {password: "***",
///        code: "print('y')"}` and a fresh `escalation_id` (S13-6, S13-8).
/// * (b) `hotl_pending` row inserted with `expires_at ≈ now + 6h`
///        (within a 1-minute tolerance for clock + scheduling jitter)
///        (S13-7).
/// * (c) `hotl_escalations` parent row inserted with the same `id` that
///        the child's `escalation_id` FK points to (S13-5).
/// * (d) Audit row's `details` JSON contains `"redaction_policy_id":
///        "<the policy id>"` (S13-6).
#[tokio::test]
async fn full_suspend_resume_with_redaction_persistence_and_per_scope_expiry() {
    let scope = "tool_call.execute_python";
    let policy_row = rule(scope, "$.password");
    let policy_id = policy_row.id;

    let store = MockEscalationStore::arc();
    let redaction_repo = StubRedactionRepo::with_rule(policy_row);
    let audit = CaptureAuditSink::arc();

    // S13-5: registry is store-backed so the suspend path triggers
    // `insert_pending` against our mock.
    let registry = Arc::new(DecisionRegistry::with_store(
        store.clone() as Arc<dyn HotlEscalationStore>
    ));

    // S13-7: per-scope-class table — `tool` → 6h. Other classes fall
    // back to `default_expiry` (24h) which we never exercise here.
    let mut expiry = HashMap::new();
    expiry.insert("tool_call".to_string(), Duration::from_secs(6 * 3600));
    let default_expiry = Duration::from_secs(24 * 3600);

    // S13-6: gate constructed with redaction repo + audit sink.
    let gate = SuspendingHotlGate::with_redaction(
        Arc::new(AlwaysEscalate),
        registry.clone(),
        default_expiry,
        expiry,
        redaction_repo,
        false,
        Some(audit.clone() as Arc<dyn HotlAuditSink>),
    );

    let args_in = json!({ "password": "x", "code": "print('y')" });

    // S13-8: verdict carries `escalation_id`, not `request_id`.
    let before_utc = Utc::now();
    let verdict = <SuspendingHotlGate as xiaoguai_agent::HotlGate>::check_with_args(
        &gate, scope, 1.0, &args_in,
    )
    .await;

    let (escalation_id, args_redacted) = match verdict {
        xiaoguai_agent::HotlGateVerdict::Suspend {
            escalation_id,
            args_redacted,
            ..
        } => (escalation_id, args_redacted),
        other => panic!("expected Suspend, got {other:?}"),
    };

    // (a) Args redacted; password masked; other fields preserved.
    assert_eq!(
        args_redacted,
        json!({ "password": "***", "code": "print('y')" }),
        "S13-6: $.password leaf must be masked while siblings survive"
    );

    // (b) Child row persisted with expires_at ≈ now + 6h.
    let child = store
        .child(escalation_id)
        .expect("S13-5: hotl_pending child row must be inserted by the gate");
    let want_expiry = before_utc + chrono::Duration::hours(6);
    let drift = (child.expires_at - want_expiry).num_seconds().abs();
    assert!(
        drift <= 60,
        "S13-7: hotl_pending.expires_at must be within 60s of now+6h; \
         got drift = {drift}s, expires_at = {expires}, expected ≈ {want}",
        expires = child.expires_at,
        want = want_expiry,
    );

    // The persisted redacted args must match the SSE payload — UI
    // restart must restore the same view.
    assert_eq!(
        child.args_redacted,
        json!({ "password": "***", "code": "print('y')" }),
        "S13-6: persisted args_redacted must match the SSE Suspend payload"
    );

    // (c) Parent row exists with the same id as the child's FK.
    let parent = store
        .parent(escalation_id)
        .expect("S13-5: hotl_escalations parent row must be inserted by the gate");
    assert_eq!(
        parent.id, child.escalation_id,
        "S13-5: child.escalation_id FK must point at the parent row id"
    );
    assert_eq!(
        parent.top_level_scope, scope,
        "S13-5: parent.top_level_scope must equal the escalation scope"
    );

    // (d) Audit row's `details` carries the matched redaction_policy_id.
    let entries = audit.entries();
    assert_eq!(
        entries.len(),
        1,
        "S13-6: exactly one hotl.escalation audit entry per Suspend verdict"
    );
    let entry = &entries[0];
    assert_eq!(entry.action, "hotl.escalation");
    assert_eq!(
        entry.details.get("redaction_policy_id"),
        Some(&serde_json::Value::String(policy_id.to_string())),
        "S13-6: details.redaction_policy_id must equal the matched rule id; got {:?}",
        entry.details,
    );

    // S13-5: the registry has exactly one live waiter for the new id.
    assert_eq!(
        registry.len(),
        1,
        "S13-5: one suspend → one live in-memory waiter"
    );
}
