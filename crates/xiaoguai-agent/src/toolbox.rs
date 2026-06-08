//! Tool catalogue used by the ReAct loop to bridge MCP servers and the LLM.
//!
//! v0.5.4 keeps the catalogue intentionally small: a flat map from tool name
//! to the owning MCP client + the original `ToolDescriptor`. Namespacing
//! across servers is the catalogue builder's responsibility — we reject
//! duplicate names at insertion time so callers can't accidentally surface
//! ambiguous tools to the model.

use std::collections::HashMap;
use std::sync::Arc;

use thiserror::Error;
use xiaoguai_llm::ToolSpec;
use xiaoguai_mcp::{McpClient, ToolDescriptor};

#[derive(Debug, Error)]
pub enum ToolboxError {
    #[error("tool {0:?} already registered")]
    Duplicate(String),
    #[error("tool {0:?} not found")]
    NotFound(String),
}

/// One registered tool — the client that owns it and the descriptor we
/// captured at registration time.
#[derive(Clone)]
pub struct ToolEntry {
    pub client: Arc<dyn McpClient>,
    pub descriptor: ToolDescriptor,
}

impl std::fmt::Debug for ToolEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolEntry")
            .field("descriptor", &self.descriptor)
            .finish_non_exhaustive()
    }
}

/// Flat registry of tools available to the agent for one run.
#[derive(Default, Debug, Clone)]
pub struct Toolbox {
    by_name: HashMap<String, ToolEntry>,
}

impl Toolbox {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-allocated builder for the common pattern of registering all tools
    /// from one MCP server at once.
    ///
    /// # Errors
    /// Returns [`ToolboxError::Duplicate`] if any tool name appears more than once.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "Arc passed by value is intentional API"
    )]
    pub fn from_server(
        client: Arc<dyn McpClient>,
        tools: Vec<ToolDescriptor>,
    ) -> Result<Self, ToolboxError> {
        let mut tb = Self::new();
        for t in tools {
            tb.insert(client.clone(), t)?;
        }
        Ok(tb)
    }

    /// # Errors
    /// Returns [`ToolboxError::Duplicate`] if `descriptor.name` is already registered.
    pub fn insert(
        &mut self,
        client: Arc<dyn McpClient>,
        descriptor: ToolDescriptor,
    ) -> Result<(), ToolboxError> {
        if self.by_name.contains_key(&descriptor.name) {
            return Err(ToolboxError::Duplicate(descriptor.name));
        }
        self.by_name
            .insert(descriptor.name.clone(), ToolEntry { client, descriptor });
        Ok(())
    }

    /// Insert, overwriting any existing entry with the same name. Unlike
    /// [`Toolbox::insert`] this never errors on a duplicate — used for
    /// built-in control tools (e.g. /loop's `loop_done`) that must take
    /// precedence over any same-named server tool rather than be shadowed.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "Arc passed by value is intentional API"
    )]
    pub fn insert_or_replace(&mut self, client: Arc<dyn McpClient>, descriptor: ToolDescriptor) {
        self.by_name
            .insert(descriptor.name.clone(), ToolEntry { client, descriptor });
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ToolEntry> {
        self.by_name.get(name)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// Render the catalogue as `ToolSpec`s the LLM backend can attach to a
    /// `ChatRequest`. Order is unspecified — the agent uses tool name as
    /// dispatch key, so order doesn't affect correctness.
    #[must_use]
    pub fn to_specs(&self) -> Vec<ToolSpec> {
        self.by_name
            .values()
            .map(|e| ToolSpec {
                name: e.descriptor.name.clone(),
                description: e.descriptor.description.clone(),
                parameters: e.descriptor.input_schema.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use xiaoguai_mcp::{McpResult, ServerInfo, ToolDescriptor, ToolResult};

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
                text: "stub".into(),
                blocks: vec![],
                is_error: false,
            })
        }
        async fn shutdown(&self) -> McpResult<()> {
            Ok(())
        }
    }

    fn td(name: &str) -> ToolDescriptor {
        ToolDescriptor {
            name: name.into(),
            description: Some(format!("tool {name}")),
            input_schema: json!({"type":"object"}),
        }
    }

    #[test]
    fn rejects_duplicate_tool_names() {
        let client: Arc<dyn McpClient> = Arc::new(StubClient);
        let mut tb = Toolbox::new();
        tb.insert(client.clone(), td("search")).unwrap();
        let err = tb.insert(client, td("search")).unwrap_err();
        assert!(matches!(err, ToolboxError::Duplicate(n) if n == "search"));
    }

    #[test]
    fn to_specs_round_trips_descriptor_fields() {
        let client: Arc<dyn McpClient> = Arc::new(StubClient);
        let tb = Toolbox::from_server(client, vec![td("a"), td("b")]).unwrap();
        let mut specs = tb.to_specs();
        specs.sort_by(|x, y| x.name.cmp(&y.name));
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "a");
        assert_eq!(specs[0].description.as_deref(), Some("tool a"));
    }
}
