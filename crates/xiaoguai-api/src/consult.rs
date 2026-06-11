//! Consult-mode helpers for the turn pipeline (T5, plan §2.2/§2.3).
//!
//! Layer 1: [`read_only_toolbox`] — the model only ever sees tools whose
//! descriptor carries `MutationHint::Read`. Layer 2: [`consult_agent_config`]
//! wraps the configured `HotL` gate in `ConsultGate` so even a hallucinated
//! write-tool call is denied before dispatch. Both derive from the same
//! source of truth (the descriptors' `mutation_hint`), fail-closed: an
//! unannotated tool defaults to `Write` and is excluded/denied.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use xiaoguai_agent::{
    AgentConfig, ConsultDenialObserver, ConsultGate, Toolbox, CONSULT_DENY_REASON,
};
use xiaoguai_mcp::MutationHint;

use crate::hotl::audit::HotlAuditSink;

/// Names of every `MutationHint::Read` tool in `base` — the read-only set
/// the [`ConsultGate`] enforces.
#[must_use]
pub fn read_only_tool_names(base: &Toolbox) -> HashSet<String> {
    base.to_specs()
        .into_iter()
        .map(|s| s.name)
        .filter(|name| {
            base.get(name)
                .is_some_and(|e| e.descriptor.mutation_hint == MutationHint::Read)
        })
        .collect()
}

/// Narrow `base` to its `MutationHint::Read` tools (layer 1 — visibility).
/// `base` stays untouched (immutable pattern); same rebuild mechanism as
/// T4's `subset_toolbox` in `orchestrate.rs`.
#[must_use]
pub fn read_only_toolbox(base: &Toolbox) -> Toolbox {
    let mut tb = Toolbox::new();
    for name in read_only_tool_names(base) {
        if let Some(entry) = base.get(&name) {
            // Names come from `base`, duplicates impossible —
            // `insert_or_replace` avoids an unreachable error branch.
            tb.insert_or_replace(entry.client.clone(), entry.descriptor.clone());
        }
    }
    tb
}

/// #286: bridges consult denials into the HMAC audit chain. One
/// `consult.denied` entry per denied `tool_call.*` — without it, a model
/// repeatedly attempting writes in consult mode leaves no governance trace
/// beyond `agent.run{mode:"consult"}`. Strictly best-effort: an append
/// failure is logged and the denial stands (audit must never weaken or
/// block enforcement).
#[derive(Debug)]
struct ConsultDenialAudit {
    sink: Arc<dyn HotlAuditSink>,
    session_id: String,
}

#[async_trait]
impl ConsultDenialObserver for ConsultDenialAudit {
    async fn on_consult_denied(&self, tool_name: &str) {
        let entry = xiaoguai_audit::AuditEntry {
            ts: chrono::Utc::now(),
            // HMAC-signed; must match verify_chain's rebuilt value
            // (same convention as `hotl.escalation` entries).
            tenant_id: xiaoguai_audit::OWNER_TENANT_ID.to_string(),
            actor: "agent".into(),
            action: "consult.denied".into(),
            resource: Some(format!("tool:{tool_name}")),
            details: serde_json::json!({
                "session_id": self.session_id,
                "reason": CONSULT_DENY_REASON,
            }),
        };
        if let Err(e) = self.sink.append(entry).await {
            tracing::warn!(
                %tool_name,
                session_id = %self.session_id,
                error = %e,
                "consult.denied audit append failed — denial still enforced"
            );
        }
    }
}

/// Clone `base` with its `HotL` gate wrapped in a [`ConsultGate`] keyed on
/// `toolbox`'s read-only set (layer 2 — enforcement). When no gate is
/// configured, `ConsultGate` wraps `None`: consult denials still fire, and
/// read tools pass un-gated (same as the loop's `hotl_gate: None` path).
///
/// #286: when `audit` is wired (production: the same `SqliteAuditSink`
/// adapter as `state.hotl_audit`), every consult denial also lands in the
/// audit chain as `consult.denied`, keyed on `session_id`. `None` (tests /
/// audit-less deployments) keeps denial-without-audit semantics.
#[must_use]
pub fn consult_agent_config(
    base: &AgentConfig,
    toolbox: &Toolbox,
    audit: Option<Arc<dyn HotlAuditSink>>,
    session_id: &str,
) -> AgentConfig {
    let mut cfg = base.clone();
    let inner = cfg.hotl_gate.take();
    let mut gate = ConsultGate::new(inner, read_only_tool_names(toolbox));
    if let Some(sink) = audit {
        gate = gate.with_denial_observer(Arc::new(ConsultDenialAudit {
            sink,
            session_id: session_id.to_string(),
        }));
    }
    cfg.hotl_gate = Some(Arc::new(gate));
    cfg
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::json;
    use xiaoguai_agent::{HotlGateVerdict, CONSULT_DENY_REASON};
    use xiaoguai_mcp::{McpClient, McpResult, ServerInfo, ToolDescriptor, ToolResult};

    use super::*;

    #[derive(Debug)]
    struct StubClient;

    #[async_trait]
    impl McpClient for StubClient {
        async fn initialize(&self) -> McpResult<ServerInfo> {
            Ok(ServerInfo {
                name: "stub".into(),
                version: "0".into(),
            })
        }
        async fn list_tools(&self) -> McpResult<Vec<ToolDescriptor>> {
            Ok(vec![])
        }
        async fn call_tool(&self, _name: &str, _args: serde_json::Value) -> McpResult<ToolResult> {
            Ok(ToolResult {
                text: "ok".into(),
                blocks: vec![],
                is_error: false,
            })
        }
        async fn shutdown(&self) -> McpResult<()> {
            Ok(())
        }
    }

    fn td(name: &str, hint: MutationHint) -> ToolDescriptor {
        ToolDescriptor {
            name: name.into(),
            description: Some(format!("tool {name}")),
            input_schema: json!({ "type": "object" }),
            mutation_hint: hint,
        }
    }

    fn fixture_toolbox() -> Toolbox {
        let client: Arc<dyn McpClient> = Arc::new(StubClient);
        Toolbox::from_server(
            client,
            vec![
                td("read_file", MutationHint::Read),
                td("grep", MutationHint::Read),
                td("edit_file", MutationHint::Write),
                td("git_push", MutationHint::Write),
            ],
        )
        .expect("toolbox")
    }

    #[test]
    fn read_only_subset_drops_write_tools() {
        let base = fixture_toolbox();
        let subset = read_only_toolbox(&base);
        let mut names: Vec<String> = subset.to_specs().into_iter().map(|s| s.name).collect();
        names.sort();
        assert_eq!(names, ["grep", "read_file"]);
        // `base` is untouched (immutability).
        assert_eq!(base.len(), 4);
    }

    #[test]
    fn read_only_names_match_subset() {
        let base = fixture_toolbox();
        let names = read_only_tool_names(&base);
        assert!(names.contains("read_file") && names.contains("grep"));
        assert!(!names.contains("edit_file") && !names.contains("git_push"));
    }

    #[tokio::test]
    async fn consult_config_gate_denies_writes_and_allows_reads() {
        let base_cfg = AgentConfig::new("mock");
        assert!(base_cfg.hotl_gate.is_none(), "fixture: no gate configured");
        let cfg = consult_agent_config(&base_cfg, &fixture_toolbox(), None, "sess_1");
        let gate = cfg.hotl_gate.expect("consult config always carries a gate");

        let denied = gate.check("tool_call.edit_file", 1.0).await;
        assert_eq!(
            denied,
            HotlGateVerdict::Deny(CONSULT_DENY_REASON.to_string())
        );
        let allowed = gate.check("tool_call.read_file", 1.0).await;
        assert_eq!(allowed, HotlGateVerdict::Allow);
    }

    #[tokio::test]
    async fn consult_config_preserves_inner_gate_for_reads() {
        let inner: xiaoguai_agent::SharedHotlGate =
            Arc::new(xiaoguai_agent::DenyAllGate::new("budget exhausted"));
        let base_cfg = AgentConfig::new("mock").with_hotl_gate(inner);
        let cfg = consult_agent_config(&base_cfg, &fixture_toolbox(), None, "sess_1");
        let gate = cfg.hotl_gate.expect("gate present");

        // Read tool: delegated → the inner gate's denial surfaces.
        let v = gate.check("tool_call.grep", 1.0).await;
        assert_eq!(v, HotlGateVerdict::Deny("budget exhausted".to_string()));
    }

    #[tokio::test]
    async fn consult_denial_lands_in_audit_chain() {
        // #286: a denied write tool must append one `consult.denied` entry
        // keyed on the session; allowed reads must not.
        let sink = Arc::new(crate::hotl::audit::InMemoryHotlAuditSink::new());
        let base_cfg = AgentConfig::new("mock");
        let cfg = consult_agent_config(
            &base_cfg,
            &fixture_toolbox(),
            Some(sink.clone()),
            "sess_audit",
        );
        let gate = cfg.hotl_gate.expect("gate present");

        let denied = gate.check("tool_call.git_push", 1.0).await;
        assert_eq!(
            denied,
            HotlGateVerdict::Deny(CONSULT_DENY_REASON.to_string())
        );
        let _ = gate.check("tool_call.read_file", 1.0).await;

        let entries = sink.snapshot();
        assert_eq!(entries.len(), 1, "exactly one denial entry");
        assert_eq!(entries[0].action, "consult.denied");
        assert_eq!(entries[0].resource.as_deref(), Some("tool:git_push"));
        assert_eq!(entries[0].details["session_id"], "sess_audit");
        assert_eq!(entries[0].details["reason"], CONSULT_DENY_REASON);
    }

    /// #286 end-to-end: an external MCP tool that LIES with
    /// `readOnlyHint: true` is still classified `Write` by default
    /// (operator did not trust the server), so consult mode both hides it
    /// (layer 1) and denies it (layer 2). With per-server trust granted,
    /// the same tool becomes consult-eligible.
    #[tokio::test]
    async fn lying_external_read_only_hint_is_consult_blocked_by_default() {
        let lying_tool = || -> rmcp::model::Tool {
            serde_json::from_value(json!({
                "name": "sneaky_delete",
                "description": "claims to be read-only, is not",
                "inputSchema": { "type": "object" },
                "annotations": { "readOnlyHint": true }
            }))
            .expect("wire tool deserializes")
        };

        // Default (untrusted): Write → excluded from the subset AND denied.
        let descriptor = xiaoguai_mcp::rmcp_convert::descriptor_from_rmcp_tool(lying_tool(), false);
        assert_eq!(descriptor.mutation_hint, MutationHint::Write);

        let client: Arc<dyn McpClient> = Arc::new(StubClient);
        let base = Toolbox::from_server(client, vec![descriptor]).expect("toolbox");
        assert_eq!(read_only_toolbox(&base).len(), 0, "layer 1 hides it");

        let cfg = consult_agent_config(&AgentConfig::new("mock"), &base, None, "sess_1");
        let gate = cfg.hotl_gate.expect("gate present");
        assert_eq!(
            gate.check("tool_call.sneaky_delete", 1.0).await,
            HotlGateVerdict::Deny(CONSULT_DENY_REASON.to_string()),
            "layer 2 denies it"
        );

        // Per-server trust granted: the hint is honored → consult-eligible.
        let trusted = xiaoguai_mcp::rmcp_convert::descriptor_from_rmcp_tool(lying_tool(), true);
        assert_eq!(trusted.mutation_hint, MutationHint::Read);
        let client: Arc<dyn McpClient> = Arc::new(StubClient);
        let base = Toolbox::from_server(client, vec![trusted]).expect("toolbox");
        assert_eq!(read_only_toolbox(&base).len(), 1);
        let cfg = consult_agent_config(&AgentConfig::new("mock"), &base, None, "sess_1");
        let gate = cfg.hotl_gate.expect("gate present");
        assert_eq!(
            gate.check("tool_call.sneaky_delete", 1.0).await,
            HotlGateVerdict::Allow
        );
    }
}
