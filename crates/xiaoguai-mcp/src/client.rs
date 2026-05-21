//! Transport-agnostic MCP client trait.

use async_trait::async_trait;
use serde_json::Value as JsonValue;

use crate::error::McpResult;
use crate::types::{ServerInfo, ToolDescriptor, ToolResult};

#[async_trait]
pub trait McpClient: Send + Sync {
    /// Returns cached server info captured during the initialize handshake.
    async fn initialize(&self) -> McpResult<ServerInfo>;

    /// List all tools exposed by the server.
    async fn list_tools(&self) -> McpResult<Vec<ToolDescriptor>>;

    /// Invoke a tool by name. `args` should be a JSON object (or null).
    async fn call_tool(&self, name: &str, args: JsonValue) -> McpResult<ToolResult>;

    /// Best-effort shutdown of the underlying transport.
    async fn shutdown(&self) -> McpResult<()>;
}

// Compile-time check that the trait stays object-safe.
const _: Option<Box<dyn McpClient>> = None;
