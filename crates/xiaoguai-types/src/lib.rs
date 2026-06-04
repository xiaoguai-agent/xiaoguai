//! Shared domain types for Xiaoguai.
//!
//! This crate is the dependency leaf — zero business logic, only data structures.
//! All other crates depend on `xiaoguai-types`, and `xiaoguai-types` depends on
//! nothing internal. Keep it that way to avoid circular dependencies.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic, clippy::nursery)]
#![allow(clippy::module_name_repetitions)]

pub mod ids;
pub mod mcp_server;
pub mod provider;
pub mod redact;
pub mod session;
pub mod tool_call;
pub mod user;

pub use ids::{
    McpServerInstanceId, MessageId, ProviderId, SessionId, ToolCallId, UserId,
};
pub use mcp_server::{McpServer, McpTransport};
pub use provider::{LlmProvider, ProviderKind};
pub use redact::redact_str;
pub use session::{ContentBlock, Message, Role as MessageRole, Session, SessionStatus};
pub use tool_call::{ToolCall, ToolCallStatus};
pub use user::{Role as TenantRole, User};
