//! `ConsultGate` — the T5 "Agent Bridge": a semantic wrapper over the
//! session's `HotL` gate that makes a turn read-only (consult mode).
//!
//! Layer 2 of the consult/execute split (plan §2.3, defense-in-depth): even
//! if a write tool somehow reaches dispatch (the model hallucinated a name,
//! or the toolbox subset missed one), the gate denies the call before the
//! MCP client is touched. Layer 1 (toolbox subsetting) lives in
//! `xiaoguai-api::turn`.
//!
//! Semantics per scope:
//! * `tool_call.{name}` where `{name}` is NOT in the read-only set →
//!   `Deny` with [`CONSULT_DENY_REASON`], **without** consulting the inner
//!   gate (no budget event, no escalation — the call was never eligible).
//! * everything else (read tools, non-`tool_call.` scopes) → delegate to
//!   the inner gate unchanged.
//!
//! The inner gate is `Option<SharedHotlGate>`: a deployment with no `HotL`
//! gate configured must STILL get consult enforcement, so `None` simply
//! means "no further gating after the consult check" (i.e. `Allow`) —
//! mirroring how the ReAct loop treats `AgentConfig::hotl_gate == None`.

use std::collections::HashSet;

use async_trait::async_trait;

use crate::hotl_gate::{HotlGate, HotlGateVerdict, SharedHotlGate};

/// Denial reason surfaced to the model (and SSE clients) when consult mode
/// blocks a write tool. Stable string — tests and UI match on it.
pub const CONSULT_DENY_REASON: &str = "consult mode: write tools are disabled";

/// Scope prefix the ReAct loop uses for per-tool-call gate checks.
const TOOL_CALL_SCOPE_PREFIX: &str = "tool_call.";

/// Read-only wrapper around the session's `HotL` gate (T5 Agent Bridge).
#[derive(Debug)]
pub struct ConsultGate {
    inner: Option<SharedHotlGate>,
    read_only_tools: HashSet<String>,
}

impl ConsultGate {
    /// `read_only_tools` is the resolved set of tool names whose descriptor
    /// carries `MutationHint::Read`. Anything not in the set is denied on
    /// `tool_call.*` scopes (fail-closed).
    #[must_use]
    pub fn new(inner: Option<SharedHotlGate>, read_only_tools: HashSet<String>) -> Self {
        Self {
            inner,
            read_only_tools,
        }
    }

    /// `Some(reason)` when consult mode must deny this scope outright.
    fn consult_denial(&self, scope: &str) -> Option<String> {
        let tool_name = scope.strip_prefix(TOOL_CALL_SCOPE_PREFIX)?;
        if self.read_only_tools.contains(tool_name) {
            None
        } else {
            Some(CONSULT_DENY_REASON.to_string())
        }
    }
}

#[async_trait]
impl HotlGate for ConsultGate {
    async fn check(&self, scope: &str, amount: f64) -> HotlGateVerdict {
        if let Some(reason) = self.consult_denial(scope) {
            return HotlGateVerdict::Deny(reason);
        }
        match &self.inner {
            Some(gate) => gate.check(scope, amount).await,
            None => HotlGateVerdict::Allow,
        }
    }

    async fn check_with_args(
        &self,
        scope: &str,
        amount: f64,
        args: &serde_json::Value,
    ) -> HotlGateVerdict {
        if let Some(reason) = self.consult_denial(scope) {
            return HotlGateVerdict::Deny(reason);
        }
        match &self.inner {
            Some(gate) => gate.check_with_args(scope, amount, args).await,
            None => HotlGateVerdict::Allow,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use serde_json::json;

    use super::*;

    /// Counting inner gate — proves delegation happened (or didn't).
    #[derive(Debug, Default)]
    struct CountingGate {
        checks: AtomicUsize,
        checks_with_args: AtomicUsize,
    }

    #[async_trait]
    impl HotlGate for CountingGate {
        async fn check(&self, _scope: &str, _amount: f64) -> HotlGateVerdict {
            self.checks.fetch_add(1, Ordering::SeqCst);
            HotlGateVerdict::Allow
        }

        async fn check_with_args(
            &self,
            _scope: &str,
            _amount: f64,
            _args: &serde_json::Value,
        ) -> HotlGateVerdict {
            self.checks_with_args.fetch_add(1, Ordering::SeqCst);
            HotlGateVerdict::Allow
        }
    }

    fn read_set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| (*s).to_string()).collect()
    }

    #[tokio::test]
    async fn write_tool_is_denied_without_touching_inner() {
        let inner = Arc::new(CountingGate::default());
        let gate = ConsultGate::new(Some(inner.clone()), read_set(&["read_file"]));

        let v = gate.check("tool_call.edit_file", 1.0).await;
        assert_eq!(v, HotlGateVerdict::Deny(CONSULT_DENY_REASON.to_string()));
        assert_eq!(inner.checks.load(Ordering::SeqCst), 0, "inner untouched");
    }

    #[tokio::test]
    async fn read_tool_is_delegated_to_inner() {
        let inner = Arc::new(CountingGate::default());
        let gate = ConsultGate::new(Some(inner.clone()), read_set(&["read_file"]));

        let v = gate.check("tool_call.read_file", 1.0).await;
        assert_eq!(v, HotlGateVerdict::Allow);
        assert_eq!(inner.checks.load(Ordering::SeqCst), 1, "inner consulted");
    }

    #[tokio::test]
    async fn non_tool_call_scope_is_delegated() {
        let inner = Arc::new(CountingGate::default());
        let gate = ConsultGate::new(Some(inner.clone()), read_set(&[]));

        let v = gate.check("llm_call", 1.0).await;
        assert_eq!(v, HotlGateVerdict::Allow);
        assert_eq!(inner.checks.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn inner_denial_on_read_tool_still_surfaces() {
        // Consult mode never weakens the inner gate: a read tool the inner
        // gate denies stays denied.
        let inner: SharedHotlGate = Arc::new(crate::hotl_gate::DenyAllGate::new("budget"));
        let gate = ConsultGate::new(Some(inner), read_set(&["read_file"]));

        let v = gate.check("tool_call.read_file", 1.0).await;
        assert_eq!(v, HotlGateVerdict::Deny("budget".to_string()));
    }

    #[tokio::test]
    async fn check_with_args_has_same_semantics() {
        let inner = Arc::new(CountingGate::default());
        let gate = ConsultGate::new(Some(inner.clone()), read_set(&["grep"]));
        let args = json!({ "pattern": "x" });

        let denied = gate.check_with_args("tool_call.git_push", 1.0, &args).await;
        assert_eq!(
            denied,
            HotlGateVerdict::Deny(CONSULT_DENY_REASON.to_string())
        );
        assert_eq!(inner.checks_with_args.load(Ordering::SeqCst), 0);

        let allowed = gate.check_with_args("tool_call.grep", 1.0, &args).await;
        assert_eq!(allowed, HotlGateVerdict::Allow);
        assert_eq!(inner.checks_with_args.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn no_inner_gate_still_enforces_consult_and_allows_reads() {
        let gate = ConsultGate::new(None, read_set(&["list_dir"]));

        let denied = gate.check("tool_call.edit_file", 1.0).await;
        assert_eq!(
            denied,
            HotlGateVerdict::Deny(CONSULT_DENY_REASON.to_string())
        );

        let allowed = gate.check("tool_call.list_dir", 1.0).await;
        assert_eq!(allowed, HotlGateVerdict::Allow);

        let other_scope = gate.check_with_args("llm_call", 1.0, &json!({})).await;
        assert_eq!(other_scope, HotlGateVerdict::Allow);
    }
}
