//! Sprint-13 S13-6 — `SuspendingHotlGate` applies `RedactionRules` before
//! emitting `HotlPending` on the SSE wire and threads the matched policy
//! `id` into the per-escalation audit row's `details` JSON.
//!
//! These integration tests pin the contract between four layers:
//!
//!   * `xiaoguai-storage::repositories::hotl_redaction::HotlRedactionRepo`
//!     (S13-3 — provides the rule rows)
//!   * `xiaoguai-auth::redaction::RedactionRules`
//!     (S13-4 — `JSONPath` mask helper)
//!   * `xiaoguai-core::hotl_bridge::SuspendingHotlGate`
//!     (S13-6 — wires the two above into the gate verdict + audit row)
//!   * `xiaoguai-api::hotl::audit::HotlAuditSink`
//!     (S12-7 — sink the audit entry lands on)
//!
//! Behaviour table this file pins:
//!
//! | rules            | required | matching rule? | gate verdict         |
//! |------------------|---------:|----------------|----------------------|
//! | one matching     |    false | yes            | Suspend (masked args)|
//! | empty            |    false | no             | Suspend (verbatim)   |
//! | empty            |     true | no             | Deny ("missing")     |
//! | one matching     |    false | yes            | + audit entry holds  |
//! |                  |          |                |   `redaction_policy_id`|
//!
//! Cross-refs: DEC-HLD-014, GR-SEC-13, lld-agent.md §4.6.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;
use xiaoguai_api::hotl::audit::HotlAuditSink;
use xiaoguai_api::hotl::decision_registry::DecisionRegistry;
use xiaoguai_api::hotl::enforcer::{HotlEnforcer, HotlVerdict, HotlVerdictResult};
use xiaoguai_audit::AuditEntry;
use xiaoguai_core::hotl_bridge::SuspendingHotlGate;
use xiaoguai_storage::repositories::error::RepoResult;
use xiaoguai_storage::repositories::hotl_redaction::{HotlRedactionRepo, RedactionPolicyRow};

// ── stubs ───────────────────────────────────────────────────────────────

/// Always returns `Escalate(reason)` so the gate exercises the suspend
/// branch (and hence the redaction logic) on every call.
#[derive(Debug)]
struct AlwaysEscalate;

#[async_trait]
impl HotlEnforcer for AlwaysEscalate {
    async fn check(&self, _scope: &str, _amount: f64) -> HotlVerdictResult {
        Ok(HotlVerdict::Escalate("test escalate".into()))
    }
}

/// In-memory `HotlRedactionRepo`. Returns the same row vector for every
/// tenant — the gate doesn't filter by tenant beyond the repo call, so
/// the test fakes don't need a multi-tenant map.
#[derive(Debug, Clone)]
struct StubRedactionRepo {
    rows: Vec<RedactionPolicyRow>,
}

impl StubRedactionRepo {
    fn empty() -> Arc<Self> {
        Arc::new(Self { rows: Vec::new() })
    }
    fn with_rule(row: RedactionPolicyRow) -> Arc<Self> {
        Arc::new(Self { rows: vec![row] })
    }
}

#[async_trait]
impl HotlRedactionRepo for StubRedactionRepo {
    async fn load_all(&self) -> RepoResult<Vec<RedactionPolicyRow>> {
        Ok(self.rows.clone())
    }
}

/// Capturing `HotlAuditSink` — collects every appended entry so tests
/// can assert on the `details` JSON.
#[derive(Debug, Default)]
struct CaptureAuditSink {
    entries: parking_lot::Mutex<Vec<AuditEntry>>,
}

impl CaptureAuditSink {
    fn new_arc() -> Arc<Self> {
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

fn rule(scope: &str, jsonpath: &str) -> RedactionPolicyRow {
    RedactionPolicyRow {
        id: Uuid::new_v4(),
        scope: scope.into(),
        jsonpath: jsonpath.into(),
        applies_to: vec!["sse".into()],
        created_at: Utc::now(),
    }
}

fn default_expiry() -> Duration {
    Duration::from_secs(60)
}

// ── tests ───────────────────────────────────────────────────────────────

/// Matching rule + `redaction_required=false` → `Suspend` arm carries
/// args with the masked field replaced by `"***"`. Verbatim password
/// must NEVER appear in the suspend payload.
#[tokio::test]
async fn gate_applies_rule_set_when_match_exists() {
    let registry = DecisionRegistry::arc();
    let enforcer: Arc<dyn HotlEnforcer> = Arc::new(AlwaysEscalate);
    let scope = "tool_call.execute_python";
    let repo = StubRedactionRepo::with_rule(rule(scope, "$.password"));
    let audit = CaptureAuditSink::new_arc();

    let gate = SuspendingHotlGate::with_redaction(
        enforcer,
        registry.clone(),
        default_expiry(),
        std::collections::HashMap::new(),
        repo,
        false, // redaction_required
        Some(audit.clone() as Arc<dyn HotlAuditSink>),
    );

    let args_in = json!({ "password": "secret", "user": "alice" });

    let verdict = <SuspendingHotlGate as xiaoguai_agent::HotlGate>::check_with_args(
        &gate, scope, 1.0, &args_in,
    )
    .await;

    let args_redacted = match verdict {
        xiaoguai_agent::HotlGateVerdict::Suspend { args_redacted, .. } => args_redacted,
        other => panic!("expected Suspend, got {other:?}"),
    };

    assert_eq!(
        args_redacted,
        json!({ "password": "***", "user": "alice" }),
        "password leaf must be masked but other fields preserved",
    );
}

/// No rules + `redaction_required=false` → verbatim args flow through.
/// This pins the v1.9.x backward-compat path (no policy → no mask).
#[tokio::test]
async fn gate_emits_verbatim_args_when_no_rule() {
    let registry = DecisionRegistry::arc();
    let enforcer: Arc<dyn HotlEnforcer> = Arc::new(AlwaysEscalate);
    let repo = StubRedactionRepo::empty();

    let gate = SuspendingHotlGate::with_redaction(
        enforcer,
        registry.clone(),
        default_expiry(),
        std::collections::HashMap::new(),
        repo,
        false, // redaction_required
        None,
    );

    let args_in = json!({ "password": "still-visible", "user": "alice" });

    let verdict = <SuspendingHotlGate as xiaoguai_agent::HotlGate>::check_with_args(
        &gate,
        "tool_call.search",
        1.0,
        &args_in,
    )
    .await;

    let args_redacted = match verdict {
        xiaoguai_agent::HotlGateVerdict::Suspend { args_redacted, .. } => args_redacted,
        other => panic!("expected Suspend, got {other:?}"),
    };
    assert_eq!(
        args_redacted, args_in,
        "without a rule the args must be returned verbatim (v1.9.x compat)"
    );
}

/// No rules + `redaction_required=true` → fail-closed `Deny`. The
/// reason MUST mention "redaction policy missing" so SREs can spot the
/// cause.
#[tokio::test]
async fn gate_fail_closed_when_required_and_no_rule() {
    let registry = DecisionRegistry::arc();
    let enforcer: Arc<dyn HotlEnforcer> = Arc::new(AlwaysEscalate);
    let repo = StubRedactionRepo::empty();

    let gate = SuspendingHotlGate::with_redaction(
        enforcer,
        registry.clone(),
        default_expiry(),
        std::collections::HashMap::new(),
        repo,
        true, // redaction_required → fail-closed
        None,
    );

    let args_in = json!({ "password": "secret" });
    let verdict = <SuspendingHotlGate as xiaoguai_agent::HotlGate>::check_with_args(
        &gate,
        "tool_call.execute_python",
        1.0,
        &args_in,
    )
    .await;

    match verdict {
        xiaoguai_agent::HotlGateVerdict::Deny(reason) => {
            assert!(
                reason.contains("redaction policy missing"),
                "Deny reason must mention 'redaction policy missing'; got: {reason}",
            );
        }
        other => panic!("expected Deny on missing policy + required, got {other:?}"),
    }
    assert!(
        registry.is_empty(),
        "fail-closed path must not register a waiter"
    );
}

/// Matching rule + audit sink → the appended audit entry carries the
/// rule's `id` under `details.redaction_policy_id`.
#[tokio::test]
async fn gate_threads_redaction_policy_id_to_audit() {
    let registry = DecisionRegistry::arc();
    let enforcer: Arc<dyn HotlEnforcer> = Arc::new(AlwaysEscalate);
    let scope = "tool_call.execute_python";
    let policy_row = rule(scope, "$.password");
    let policy_id = policy_row.id;
    let repo = StubRedactionRepo::with_rule(policy_row);
    let audit = CaptureAuditSink::new_arc();

    let gate = SuspendingHotlGate::with_redaction(
        enforcer,
        registry.clone(),
        default_expiry(),
        std::collections::HashMap::new(),
        repo,
        false,
        Some(audit.clone() as Arc<dyn HotlAuditSink>),
    );

    let args_in = json!({ "password": "secret" });
    let _verdict = <SuspendingHotlGate as xiaoguai_agent::HotlGate>::check_with_args(
        &gate, scope, 1.0, &args_in,
    )
    .await;

    let entries = audit.entries();
    assert_eq!(
        entries.len(),
        1,
        "exactly one audit entry must be appended per escalation; got {entries:?}",
    );
    let entry = &entries[0];
    assert_eq!(
        entry.action, "hotl.escalation",
        "escalation audit action must be hotl.escalation"
    );
    let got_id = entry
        .details
        .get("redaction_policy_id")
        .expect("details must include redaction_policy_id");
    assert_eq!(
        got_id,
        &serde_json::Value::String(policy_id.to_string()),
        "redaction_policy_id in audit details must match the matched rule's id",
    );
}
