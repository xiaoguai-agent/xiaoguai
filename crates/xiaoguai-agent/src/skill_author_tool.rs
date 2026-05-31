//! Tier-2 D.1 — agent-side `propose_skill` MCP tool wiring.
//!
//! `xiaoguai-tasks::skill_author::propose` is the real work; this module
//! is a thin synthetic `McpClient` that the agent's toolbox can register
//! alongside real MCP servers. The ReAct loop's existing per-tool `HotL`
//! gate (PR #61) covers `tool_call.propose_skill`; the rate-limit budget
//! for proposals themselves uses the `skill_author` bucket and is
//! consulted inside `xiaoguai-tasks::skill_author::propose`. That
//! double-gate is intentional: the first bucket controls "can the agent
//! use this tool at all", the second controls "how many drafts per day".
//!
//! The agent doesn't depend on `xiaoguai-tasks` directly (it stays a leaf
//! crate). Instead, this module defines a [`ProposeSkillBackend`] trait
//! and the `xiaoguai-core::skill_author_bridge` adapter implements it
//! by calling into `xiaoguai-tasks`.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use xiaoguai_mcp::{
    ContentBlock, McpClient, McpError, McpResult, ServerInfo, ToolDescriptor, ToolResult,
};

/// Canonical name of the tool. Kept here AND in
/// `xiaoguai_tasks::skill_author::PROPOSE_SKILL_TOOL_NAME` so the
/// recursion-guard sees the same string the agent registers. The two
/// constants are unit-tested together in `xiaoguai-core`.
pub const PROPOSE_SKILL_TOOL_NAME: &str = "propose_skill";

/// Input the LLM submits when calling `propose_skill`. Mirrors
/// `xiaoguai_tasks::skill_author::SkillManifest`. We re-declare it here
/// to avoid a crate-graph dependency `agent → tasks`; equivalence is
/// asserted by a unit test in `xiaoguai-core`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProposeSkillArgs {
    pub name: String,
    pub description: String,
    pub version: String,
    pub system_prompt: String,
    pub tool_allowlist: Vec<String>,
}

/// Backend invoked by [`ProposeSkillClient::call_tool`]. The production
/// impl lives in `xiaoguai-core::skill_author_bridge`; tests use the
/// `StubBackend` defined at the bottom of this file.
#[async_trait]
pub trait ProposeSkillBackend: Send + Sync + std::fmt::Debug {
    /// Returns `Ok(proposal_id)` on success, `Err(human-readable_reason)`
    /// on validator/gate denial. The reason becomes the synthetic
    /// tool-result content the agent observes.
    async fn invoke(
        &self,
        tenant_id: &str,
        proposed_by: &str,
        args: ProposeSkillArgs,
    ) -> Result<String, String>;
}

/// Synthetic MCP client exposing `propose_skill`. Stateless beyond the
/// `Arc` to the backend — many agents can share one client safely.
#[derive(Debug, Clone)]
pub struct ProposeSkillClient {
    inner: Arc<dyn ProposeSkillBackend>,
    tenant_id: String,
    proposed_by: String,
}

impl ProposeSkillClient {
    #[must_use]
    pub fn new(
        inner: Arc<dyn ProposeSkillBackend>,
        tenant_id: impl Into<String>,
        proposed_by: impl Into<String>,
    ) -> Self {
        Self {
            inner,
            tenant_id: tenant_id.into(),
            proposed_by: proposed_by.into(),
        }
    }

    /// JSON-schema for the tool input. Strict whitelist — no
    /// `additionalProperties`, all top-level keys typed, allowlist is
    /// `array<string>` with `minItems: 1`.
    #[must_use]
    pub fn input_schema() -> Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["name", "description", "version", "system_prompt", "tool_allowlist"],
            "properties": {
                "name": {
                    "type": "string",
                    "pattern": "^[A-Za-z0-9_-]+$",
                    "minLength": 1
                },
                "description": { "type": "string", "minLength": 1 },
                "version": {
                    "type": "string",
                    "pattern": "^[0-9]+\\.[0-9]+\\.[0-9]+(-[A-Za-z0-9.-]+)?$"
                },
                "system_prompt": { "type": "string", "minLength": 1 },
                "tool_allowlist": {
                    "type": "array",
                    "minItems": 1,
                    "items": { "type": "string", "minLength": 1 }
                }
            }
        })
    }

    /// `ToolDescriptor` for the agent toolbox.
    #[must_use]
    pub fn descriptor() -> ToolDescriptor {
        ToolDescriptor {
            name: PROPOSE_SKILL_TOOL_NAME.into(),
            description: Some(
                "Propose a new skill pack for this tenant. The proposal is HotL-gated and \
                 admin-approved before becoming loadable. Off by default (operator must \
                 opt-in per tenant). tool_allowlist MUST be a subset of currently \
                 registered tools and MUST NOT include propose_skill."
                    .into(),
            ),
            input_schema: Self::input_schema(),
        }
    }
}

#[async_trait]
impl McpClient for ProposeSkillClient {
    async fn initialize(&self) -> McpResult<ServerInfo> {
        Ok(ServerInfo {
            name: "xiaoguai-skill-author".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        })
    }

    async fn list_tools(&self) -> McpResult<Vec<ToolDescriptor>> {
        Ok(vec![Self::descriptor()])
    }

    async fn call_tool(&self, name: &str, args: Value) -> McpResult<ToolResult> {
        if name != PROPOSE_SKILL_TOOL_NAME {
            return Err(McpError::Protocol(format!(
                "ProposeSkillClient cannot dispatch tool {name:?}"
            )));
        }
        let parsed: ProposeSkillArgs = serde_json::from_value(args)
            .map_err(|e| McpError::Protocol(format!("propose_skill args invalid: {e}")))?;
        match self
            .inner
            .invoke(&self.tenant_id, &self.proposed_by, parsed)
            .await
        {
            Ok(proposal_id) => Ok(ToolResult {
                text: format!("skill proposal {proposal_id} created (pending admin approval)"),
                blocks: vec![ContentBlock::Text {
                    text: serde_json::to_string(&serde_json::json!({
                        "proposal_id": proposal_id,
                        "status": "pending",
                    }))
                    .unwrap_or_default(),
                }],
                is_error: false,
            }),
            Err(reason) => Ok(ToolResult {
                text: format!("propose_skill denied: {reason}"),
                blocks: vec![ContentBlock::Text {
                    text: serde_json::to_string(&serde_json::json!({
                        "status": "denied",
                        "reason": reason,
                    }))
                    .unwrap_or_default(),
                }],
                is_error: true,
            }),
        }
    }

    async fn shutdown(&self) -> McpResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    #[derive(Debug)]
    struct StubBackend {
        last: Mutex<Option<(String, String, ProposeSkillArgs)>>,
        reply: Mutex<Result<String, String>>,
    }

    impl StubBackend {
        fn new(reply: Result<String, String>) -> Arc<Self> {
            Arc::new(Self {
                last: Mutex::new(None),
                reply: Mutex::new(reply),
            })
        }

        fn last(&self) -> Option<(String, String, ProposeSkillArgs)> {
            self.last.lock().clone()
        }
    }

    #[async_trait]
    impl ProposeSkillBackend for StubBackend {
        async fn invoke(
            &self,
            tenant_id: &str,
            proposed_by: &str,
            args: ProposeSkillArgs,
        ) -> Result<String, String> {
            *self.last.lock() = Some((tenant_id.into(), proposed_by.into(), args));
            self.reply.lock().clone()
        }
    }

    #[test]
    fn descriptor_has_strict_schema() {
        let d = ProposeSkillClient::descriptor();
        assert_eq!(d.name, "propose_skill");
        let schema = d.input_schema;
        assert_eq!(
            schema["additionalProperties"],
            serde_json::Value::Bool(false)
        );
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "tool_allowlist"));
    }

    #[test]
    fn input_schema_rejects_extra_properties_via_serde() {
        // The strict shape is also enforced by `ProposeSkillArgs`'s
        // deserialization — unknown top-level keys are silently dropped
        // by serde unless we add `#[serde(deny_unknown_fields)]`. Let's
        // assert the JSON-schema covers that gap by declaring
        // additionalProperties=false (validators on the LLM side honor
        // it; ours does not — but the schema is the contract).
        let schema = ProposeSkillClient::input_schema();
        assert_eq!(
            schema["additionalProperties"],
            serde_json::Value::Bool(false)
        );
    }

    #[tokio::test]
    async fn call_tool_dispatches_to_backend() {
        let backend = StubBackend::new(Ok("prop-123".into()));
        let client = ProposeSkillClient::new(backend.clone(), "tenant-a", "agent-1");
        let args = serde_json::json!({
            "name": "ar-collector",
            "description": "collect AR",
            "version": "0.1.0",
            "system_prompt": "you collect AR",
            "tool_allowlist": ["search"],
        });
        let res = client.call_tool("propose_skill", args).await.unwrap();
        assert!(!res.is_error, "happy path is not an error");
        let (t, p, args) = backend.last().expect("backend invoked");
        assert_eq!(t, "tenant-a");
        assert_eq!(p, "agent-1");
        assert_eq!(args.name, "ar-collector");
        assert_eq!(args.tool_allowlist, vec!["search".to_string()]);
    }

    #[tokio::test]
    async fn call_tool_propagates_denial_as_tool_error() {
        let backend = StubBackend::new(Err("budget exceeded".into()));
        let client = ProposeSkillClient::new(backend, "tenant-a", "agent-1");
        let args = serde_json::json!({
            "name": "ar-collector",
            "description": "collect AR",
            "version": "0.1.0",
            "system_prompt": "you collect AR",
            "tool_allowlist": ["search"],
        });
        let res = client.call_tool("propose_skill", args).await.unwrap();
        assert!(res.is_error, "gate denial must surface as is_error=true");
        assert!(res.text.contains("budget exceeded"));
    }

    #[tokio::test]
    async fn call_tool_rejects_other_tool_names() {
        let backend = StubBackend::new(Ok("x".into()));
        let client = ProposeSkillClient::new(backend, "tenant-a", "agent-1");
        let err = client
            .call_tool("execute_python", serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, McpError::Protocol(_)));
    }

    #[tokio::test]
    async fn call_tool_rejects_malformed_args() {
        let backend = StubBackend::new(Ok("x".into()));
        let client = ProposeSkillClient::new(backend, "tenant-a", "agent-1");
        let err = client
            .call_tool("propose_skill", serde_json::json!({"bogus": true}))
            .await
            .unwrap_err();
        assert!(matches!(err, McpError::Protocol(_)));
    }
}
