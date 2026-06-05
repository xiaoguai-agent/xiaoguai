//! Bridges the governed coding workflow (`xiaoguai-coding`, DEC-034/035) onto
//! the real moat: the HMAC audit chain (`SqliteAuditSink`, DEC-004) and the
//! `HotL` gate (DEC-006). Mirrors `audit_bridge.rs` / `skill_author_bridge.rs` —
//! the coding crate stays storage-/gate-agnostic via its `StepRecorder` /
//! `CodingGate` traits, and these concrete impls live in `xiaoguai-core` where
//! the sink and gate are already constructed.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::json;
use xiaoguai_agent::{HotlGate, HotlGateVerdict};
use xiaoguai_audit::chain::sink::SqliteAuditSink;
use xiaoguai_audit::{AuditEntry, OWNER_TENANT_ID};
use xiaoguai_coding::{CodingGate, CodingStep, GateDecision, StepRecorder};

/// Each coding tool call gates as a single unit of work (matching the agent
/// loop's per-tool-call locality); coding actions carry no spend, so the gate
/// `amount` is a nominal 1.0.
const CODING_GATE_AMOUNT: f64 = 1.0;

/// Map a [`CodingStep`] onto a chain [`AuditEntry`]. Pure (takes `now`) so the
/// field mapping is unit-testable without a live sink. `tenant_id` is the audit
/// OWNER constant so the row verifies against `verify_chain` (DEC-033 carve-out).
fn step_to_entry(step: &CodingStep, now: DateTime<Utc>) -> AuditEntry {
    AuditEntry {
        ts: now,
        tenant_id: OWNER_TENANT_ID.to_string(),
        actor: "agent".to_string(),
        action: step.action.clone(),
        resource: Some(format!("workspace:{}", step.workspace_id)),
        details: json!({
            "scope": step.scope,
            "checkpoint": step.checkpoint,
            "summary": step.summary,
        }),
    }
}

/// [`StepRecorder`] that appends coding steps to the HMAC audit chain. An
/// append failure degrades to a `warn` and never blocks the coding operation
/// (project audit-resilience rule).
#[derive(Clone)]
pub struct AuditStepRecorder {
    sink: Arc<SqliteAuditSink>,
}

impl AuditStepRecorder {
    #[must_use]
    pub fn new(sink: Arc<SqliteAuditSink>) -> Self {
        Self { sink }
    }
}

#[async_trait]
impl StepRecorder for AuditStepRecorder {
    async fn record(&self, step: CodingStep) {
        let entry = step_to_entry(&step, Utc::now());
        if let Err(err) = self.sink.append(entry).await {
            tracing::warn!(
                action = %step.action,
                workspace = %step.workspace_id,
                %err,
                "coding audit append failed; continuing (operation already applied)"
            );
        }
    }
}

/// [`CodingGate`] over the real `HotL` gate (DEC-006).
///
/// The agent loop owns the `Suspend`/resume lifecycle (`DecisionRegistry` + SSE);
/// by the contract of `xiaoguai-coding::GovernedTools`, a verdict reaching this
/// bridge must already be resolved. If a `Suspend` verdict is nonetheless
/// produced (a suspending gate used outside the loop), it is mapped
/// conservatively to `Deny` — the coding mutation must not proceed without an
/// approval this context cannot obtain.
#[derive(Clone)]
pub struct HotlCodingGate {
    gate: Arc<dyn HotlGate>,
}

impl HotlCodingGate {
    #[must_use]
    pub fn new(gate: Arc<dyn HotlGate>) -> Self {
        Self { gate }
    }
}

#[async_trait]
impl CodingGate for HotlCodingGate {
    async fn decide(&self, scope: &str) -> GateDecision {
        match self.gate.check(scope, CODING_GATE_AMOUNT).await {
            HotlGateVerdict::Allow => GateDecision::Allow,
            HotlGateVerdict::Deny(reason) => GateDecision::Deny(reason),
            HotlGateVerdict::Suspend { escalation_id, .. } => {
                tracing::warn!(
                    %scope,
                    %escalation_id,
                    "coding gate received Suspend outside the loop; denying (no approver in context)"
                );
                GateDecision::Deny(
                    "requires interactive approval not available in this context".to_string(),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use xiaoguai_agent::{AllowAllGate, ScopeDenyGate};

    #[test]
    fn step_to_entry_maps_action_resource_and_details() {
        let step = CodingStep {
            action: "code.edit".into(),
            workspace_id: "ws-123".into(),
            scope: "tool_call.edit_file".into(),
            checkpoint: Some("abc123".into()),
            summary: "src/main.rs (+1 repl)".into(),
        };
        let entry = step_to_entry(&step, Utc::now());

        assert_eq!(entry.tenant_id, OWNER_TENANT_ID);
        assert_eq!(entry.action, "code.edit");
        assert_eq!(entry.resource.as_deref(), Some("workspace:ws-123"));
        assert_eq!(entry.details["scope"], "tool_call.edit_file");
        assert_eq!(entry.details["checkpoint"], "abc123");
        assert_eq!(entry.details["summary"], "src/main.rs (+1 repl)");
    }

    #[test]
    fn denied_step_carries_null_checkpoint() {
        let step = CodingStep {
            action: "code.edit_denied".into(),
            workspace_id: "ws-1".into(),
            scope: "tool_call.edit_file".into(),
            checkpoint: None,
            summary: "blocked".into(),
        };
        let entry = step_to_entry(&step, Utc::now());
        assert!(entry.details["checkpoint"].is_null());
        assert_eq!(entry.action, "code.edit_denied");
    }

    #[tokio::test]
    async fn hotl_gate_allow_maps_to_allow() {
        let gate = HotlCodingGate::new(Arc::new(AllowAllGate));
        assert_eq!(
            gate.decide("tool_call.edit_file").await,
            GateDecision::Allow
        );
    }

    #[tokio::test]
    async fn hotl_gate_deny_maps_to_deny() {
        let gate = HotlCodingGate::new(Arc::new(ScopeDenyGate::new(
            vec!["tool_call.edit_file".to_string()],
            "no edits in this context",
        )));
        assert!(matches!(
            gate.decide("tool_call.edit_file").await,
            GateDecision::Deny(_)
        ));
    }
}
