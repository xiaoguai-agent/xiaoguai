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

use xiaoguai_agent::{AgentConfig, ConsultGate, Toolbox};
use xiaoguai_mcp::MutationHint;

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

/// Clone `base` with its `HotL` gate wrapped in a [`ConsultGate`] keyed on
/// `toolbox`'s read-only set (layer 2 — enforcement). When no gate is
/// configured, `ConsultGate` wraps `None`: consult denials still fire, and
/// read tools pass un-gated (same as the loop's `hotl_gate: None` path).
#[must_use]
pub fn consult_agent_config(base: &AgentConfig, toolbox: &Toolbox) -> AgentConfig {
    let mut cfg = base.clone();
    let inner = cfg.hotl_gate.take();
    cfg.hotl_gate = Some(Arc::new(ConsultGate::new(
        inner,
        read_only_tool_names(toolbox),
    )));
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
        let cfg = consult_agent_config(&base_cfg, &fixture_toolbox());
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
        let cfg = consult_agent_config(&base_cfg, &fixture_toolbox());
        let gate = cfg.hotl_gate.expect("gate present");

        // Read tool: delegated → the inner gate's denial surfaces.
        let v = gate.check("tool_call.grep", 1.0).await;
        assert_eq!(v, HotlGateVerdict::Deny("budget exhausted".to_string()));
    }
}
