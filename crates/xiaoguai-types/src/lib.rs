//! Shared domain types for Xiaoguai.
//!
//! This crate is the dependency leaf — zero business logic, only data structures.
//! All other crates depend on `xiaoguai-types`, and `xiaoguai-types` depends on
//! nothing internal. Keep it that way to avoid circular dependencies.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic, clippy::nursery)]
#![allow(clippy::module_name_repetitions)]

pub mod ids;
pub mod session;
pub mod tenant;
pub mod tool_call;

pub use ids::{
    McpServerInstanceId, MessageId, ProviderId, SessionId, TenantId, ToolCallId, UserId,
};
pub use session::{ContentBlock, Message, Role as MessageRole, Session, SessionStatus};
pub use tenant::{Role as TenantRole, Tenant, TenantStatus, User};
pub use tool_call::{ToolCall, ToolCallStatus};
