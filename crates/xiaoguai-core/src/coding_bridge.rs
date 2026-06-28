//! Bridges the governed coding workflow (`xiaoguai-coding`, DEC-034/035) onto
//! the real moat: the HMAC audit chain (`SqliteAuditSink`, DEC-004) and the
//! `HotL` gate (DEC-006). Mirrors `audit_bridge.rs` / `skill_author_bridge.rs` —
//! the coding crate stays storage-/gate-agnostic via its `StepRecorder` /
//! `CodingGate` traits, and these concrete impls live in `xiaoguai-core` where
//! the sink and gate are already constructed.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::json;
use xiaoguai_agent::{AllowAllGate, HotlGate, HotlGateVerdict, Toolbox};
use xiaoguai_audit::chain::sink::SqliteAuditSink;
use xiaoguai_audit::{AuditEntry, OWNER_TENANT_ID};
use xiaoguai_coding::{
    coding_tool_descriptors, CodingGate, CodingMcpClient, CodingStep, GateDecision, GovernedTools,
    StepRecorder, Workspace,
};
use xiaoguai_mcp::McpClient;

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

/// Build the governed coding tools over the workspace at `root` and register
/// them into `toolbox` so the ReAct loop surfaces them to the model.
///
/// The coding gate is **allow-all**: the loop already enforces the real `HotL`
/// decision on each `tool_call.<name>` scope before dispatch, so re-gating in
/// `GovernedTools` would be double-gating. What this layer still provides is the
/// pre-mutation checkpoint (for rollback) and the `code.*` / `git.*` audit rows
/// carrying the checkpoint id — the half of the trust coin the generic loop
/// audit doesn't.
///
/// `include_egress` exposes the network/past-undo tools (`git_push`,
/// `open_pr`); keep it `false` unless the operator explicitly opts in.
///
/// # Errors
/// Returns an error if the workspace cannot be opened/initialised, or if a tool
/// name collides (cannot happen with a fresh toolbox — defensive).
pub async fn build_coding_toolbox(
    sink: Arc<SqliteAuditSink>,
    root: &Path,
    include_egress: bool,
) -> Result<Toolbox> {
    let workspace = Workspace::open_or_create(root)
        .await
        .with_context(|| format!("open coding workspace at {}", root.display()))?;
    let tools = GovernedTools::new(
        workspace,
        HotlCodingGate::new(Arc::new(AllowAllGate)),
        AuditStepRecorder::new(sink),
    );
    let client: Arc<dyn McpClient> = Arc::new(CodingMcpClient::new(tools, include_egress));
    // All-or-nothing: build into a fresh toolbox so a mid-loop collision can
    // never leave a half-exposed coding surface (security-review M1).
    let mut toolbox = Toolbox::new();
    for descriptor in coding_tool_descriptors(include_egress) {
        let name = descriptor.name.clone();
        toolbox
            .insert(client.clone(), descriptor)
            .with_context(|| format!("register coding tool {name}"))?;
    }
    Ok(toolbox)
}

/// The coding workspace root, or `None` when coding is **not enabled**.
///
/// There is deliberately **no default**: coding tools are opt-in. An operator
/// enables governed in-loop coding by pointing `XIAOGUAI_CODING_WORKSPACE` at a
/// directory; when unset the server registers no coding tools and never
/// `git init`s its working directory (security-review H1).
#[must_use]
pub fn coding_workspace_root() -> Option<std::path::PathBuf> {
    std::env::var_os("XIAOGUAI_CODING_WORKSPACE")
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// Feature ⑤ — resolve the coding workspace root for a single turn, honouring
/// the session's per-session override.
///
/// When `session_working_dir` is `Some(path)` and non-empty, that absolute
/// server path is the workspace root for this turn (the coding tools'
/// file-write / output base). Otherwise we fall back to the global default
/// resolved by [`coding_workspace_root`] (`XIAOGUAI_CODING_WORKSPACE`), so a
/// session that pins no directory behaves exactly as before.
///
/// This only changes **which root** is used; the opt-in gating and security
/// model are unchanged — when the global default is also unset the result is
/// `None` and no coding tools are registered, exactly as today.
#[must_use]
pub fn coding_workspace_root_for_session(
    session_working_dir: Option<&str>,
) -> Option<std::path::PathBuf> {
    match session_working_dir.map(str::trim).filter(|s| !s.is_empty()) {
        Some(dir) => Some(std::path::PathBuf::from(dir)),
        None => coding_workspace_root(),
    }
}

/// Whether the **egress** coding tools (`git_push`, `open_pr`) are exposed —
/// off unless `XIAOGUAI_CODING_ALLOW_EGRESS` is truthy (`1`/`true`/`yes`). They
/// leave the local machine and cannot be rolled back, so they require a second,
/// explicit opt-in on top of enabling coding (security-review C1).
#[must_use]
pub fn coding_allow_egress() -> bool {
    std::env::var("XIAOGUAI_CODING_ALLOW_EGRESS").is_ok_and(|v| {
        let v = v.trim().to_ascii_lowercase();
        v == "1" || v == "true" || v == "yes"
    })
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

    #[test]
    fn session_working_dir_override_wins() {
        // A non-empty per-session dir is used verbatim — that's the whole
        // point of Feature ⑤.
        let root = coding_workspace_root_for_session(Some("/srv/work/sess-1"));
        assert_eq!(
            root.as_deref(),
            Some(std::path::Path::new("/srv/work/sess-1"))
        );
    }

    #[test]
    fn session_working_dir_trims_and_treats_blank_as_unset() {
        // Surrounding whitespace is trimmed; a blank override is treated as
        // "no override" and falls through to the global default. With no
        // XIAOGUAI_CODING_WORKSPACE set in the test env that default is None.
        assert_eq!(
            coding_workspace_root_for_session(Some("   ")),
            coding_workspace_root()
        );
        let trimmed = coding_workspace_root_for_session(Some("  /srv/x  "));
        assert_eq!(trimmed.as_deref(), Some(std::path::Path::new("/srv/x")));
    }

    #[test]
    fn no_session_dir_falls_back_to_global_default() {
        // None override ⇒ identical to the global resolver (opt-in gating
        // unchanged: still None when the env var is unset).
        assert_eq!(
            coding_workspace_root_for_session(None),
            coding_workspace_root()
        );
    }
}
