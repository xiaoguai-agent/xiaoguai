//! MCP client + per-tenant supervisor.
//!
//! v0.5.3 shipped stdio + supervisor lifecycle. v0.9.0 adds the
//! Streamable-HTTP transport (modern MCP HTTP spec, also handles
//! SSE-streamed responses) so the supervisor can host community servers
//! that don't ship a stdio entry point.
//!
//! Ships:
//!   - `McpClient` trait + `StdioMcpClient` (stdio via `TokioChildProcess`)
//!   - `HttpMcpClient` (Streamable HTTP via reqwest; v0.9.0)
//!   - `McpServer` domain type + PG repository (in `xiaoguai-storage`)
//!   - `McpSupervisor` minimal lifecycle (`start`/`get`/`stop`/`list_active`)
//!
//! Still deferred: cgroup+seccomp+netns sandbox, ping-based health
//! checks, default-deny network policy.
//!
//! v1.3.3-prep: adds `servers::github_pr` — in-process GitHub REST adapter
//! used by the pr-review pack.

#![forbid(unsafe_code)]

pub mod auth;
pub mod client;
pub mod error;
pub mod http;
pub mod servers;
pub mod stdio;
pub mod supervisor;
pub mod types;

pub use auth::{
    AuthConfig, InMemoryTokenStore, OAuth2PkceConfig, PkcePair, TokenBundle, TokenStore,
};
pub use client::McpClient;
pub use error::{McpError, McpResult};
pub use http::{HttpClientConfig, HttpMcpClient};
pub use stdio::StdioMcpClient;
pub use supervisor::{McpKey, McpSupervisor};
pub use types::{ContentBlock, ServerInfo, ToolDescriptor, ToolResult};
