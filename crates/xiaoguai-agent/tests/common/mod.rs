//! Shared test helpers for the agent integration suite.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use xiaoguai_mcp::{McpClient, McpResult, ServerInfo, ToolDescriptor, ToolResult};

/// One scripted tool response.
///
/// `dead_code` is allowed because integration tests live in separate
/// compilation units — a per-target check may only see one variant used.
#[derive(Clone)]
#[allow(dead_code)]
pub enum ToolResponse {
    Ok(String),
    Err(String),
    /// Sleep `Duration` then succeed. Used to exercise parallel-dispatch
    /// behaviour — total wall time should be `max(sleeps)`, not sum.
    Delayed(Duration, String),
}

/// In-memory MCP client backed by a scripted response table per tool name.
/// Call counts are accumulated so tests can assert on invocation order.
pub struct MockMcpClient {
    responses: Mutex<HashMap<String, Vec<ToolResponse>>>,
    pub call_log: Mutex<Vec<(String, JsonValue)>>,
    pub descriptors: Vec<ToolDescriptor>,
}

impl std::fmt::Debug for MockMcpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockMcpClient")
            .field(
                "tools",
                &self
                    .descriptors
                    .iter()
                    .map(|d| d.name.clone())
                    .collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl MockMcpClient {
    pub fn new(tools: Vec<(&str, ToolResponse)>) -> Arc<Self> {
        let mut responses: HashMap<String, Vec<ToolResponse>> = HashMap::new();
        let mut descriptors = Vec::new();
        for (name, resp) in tools {
            responses.entry(name.to_string()).or_default().push(resp);
            descriptors.push(ToolDescriptor {
                name: name.to_string(),
                description: Some(format!("mock tool {name}")),
                input_schema: json!({"type":"object"}),
                mutation_hint: xiaoguai_mcp::MutationHint::default(),
            });
        }
        // De-dup descriptors by name (keep first).
        let mut seen = std::collections::HashSet::new();
        descriptors.retain(|d| seen.insert(d.name.clone()));
        Arc::new(Self {
            responses: Mutex::new(responses),
            call_log: Mutex::new(Vec::new()),
            descriptors,
        })
    }

    pub fn call_count(&self, name: &str) -> usize {
        self.call_log
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _)| n == name)
            .count()
    }
}

#[async_trait]
impl McpClient for MockMcpClient {
    async fn initialize(&self) -> McpResult<ServerInfo> {
        Ok(ServerInfo {
            name: "mock-mcp".into(),
            version: "0.0.0".into(),
        })
    }

    async fn list_tools(&self) -> McpResult<Vec<ToolDescriptor>> {
        Ok(self.descriptors.clone())
    }

    async fn call_tool(&self, name: &str, args: JsonValue) -> McpResult<ToolResult> {
        self.call_log
            .lock()
            .unwrap()
            .push((name.to_string(), args.clone()));
        let resp = {
            let mut map = self.responses.lock().unwrap();
            let q = map.get_mut(name).ok_or_else(|| {
                xiaoguai_mcp::McpError::Protocol(format!("unknown mock tool: {name}"))
            })?;
            if q.is_empty() {
                return Err(xiaoguai_mcp::McpError::Protocol(format!(
                    "mock tool {name} ran out of scripted responses"
                )));
            }
            if q.len() == 1 {
                q[0].clone()
            } else {
                q.remove(0)
            }
        };
        match resp {
            ToolResponse::Ok(s) => Ok(ToolResult {
                text: s,
                blocks: vec![],
                is_error: false,
            }),
            ToolResponse::Err(s) => Ok(ToolResult {
                text: s,
                blocks: vec![],
                is_error: true,
            }),
            ToolResponse::Delayed(d, s) => {
                tokio::time::sleep(d).await;
                Ok(ToolResult {
                    text: s,
                    blocks: vec![],
                    is_error: false,
                })
            }
        }
    }

    async fn shutdown(&self) -> McpResult<()> {
        Ok(())
    }
}
