//! Repository pattern: business code only sees these traits, never raw SQL.
//!
//! Each entity has its own module + trait + `SQLite` impl. Single-owner
//! deployment (DEC-033): no tenants, no row-level security.

pub mod error;
pub mod hotl_escalations;
pub mod hotl_redaction;
pub mod im;
pub mod llm_provider;
pub mod mcp_server;
pub mod message;
pub mod session;
pub mod token_usage;
pub mod user;

pub use error::{RepoError, RepoResult};
pub use hotl_escalations::{
    HotlDecisionVerdict, HotlEscalationRow, HotlEscalationStore, HotlPendingRow,
    SqliteHotlEscalationRepository,
};
pub use hotl_redaction::{HotlRedactionRepo, RedactionPolicyRow, SqliteHotlRedactionRepo};
pub use im::{
    ExternalConversation, ExternalIdentity, ImConversation, ImIdentity, ImIdentityRepository,
    SqliteImIdentityRepository,
};
pub use llm_provider::{LlmProviderRepository, SqliteLlmProviderRepository};
pub use mcp_server::{McpServerRepository, SqliteMcpServerRepository};
pub use message::{MessageRepository, SqliteMessageRepository};
pub use session::{SessionRepository, SqliteSessionRepository};
pub use token_usage::{
    SqliteTokenUsageRepository, StoredTokenUsage, TokenUsageEntry, TokenUsageRepository,
};
pub use user::{SqliteUserRepository, UserRepository};
