//! MCP client + per-tenant supervisor.
//!
//! v0.5.3 ships:
//!   - `McpClient` trait + `StdioMcpClient` (stdio transport via rmcp)
//!   - `McpServer` domain type + PG repository (in `xiaoguai-storage`)
//!   - `McpSupervisor` minimal lifecycle (`start`/`get`/`stop`/`list_active`)
//!
//! Deferred to v0.5.3.1: SSE/HTTP transports, cgroup+seccomp+netns sandbox,
//! ping-based health checks, default-deny network policy.

#![forbid(unsafe_code)]

pub mod client;
pub mod error;
pub mod stdio;
pub mod types;

pub use client::McpClient;
pub use error::{McpError, McpResult};
pub use stdio::StdioMcpClient;
pub use types::{ContentBlock, ServerInfo, ToolDescriptor, ToolResult};
