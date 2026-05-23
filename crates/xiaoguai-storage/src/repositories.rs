//! Repository pattern: business code only sees these traits, never raw SQL.
//!
//! Each entity has its own module + trait + Postgres impl. RLS is enforced
//! defense-in-depth via Postgres policies (ADR — multi-tenant isolation).

pub mod error;
pub mod im;
pub mod llm_provider;
pub mod mcp_server;
pub mod message;
pub mod session;
pub mod tenant;
pub mod tenant_ctx;
pub mod token_usage;
pub mod user;

pub use error::{RepoError, RepoResult};
pub use im::{
    ExternalConversation, ExternalIdentity, ImConversation, ImIdentity, ImIdentityRepository,
    PgImIdentityRepository,
};
pub use llm_provider::{LlmProviderRepository, PgLlmProviderRepository};
pub use mcp_server::{McpServerRepository, PgMcpServerRepository};
pub use message::{MessageRepository, PgMessageRepository};
pub use session::{PgSessionRepository, SessionRepository};
pub use tenant::{PgTenantRepository, TenantRepository};
pub use tenant_ctx::begin_tenant_tx;
pub use token_usage::{
    PgTokenUsageRepository, StoredTokenUsage, TokenUsageEntry, TokenUsageRepository,
};
pub use user::{PgUserRepository, UserRepository};
