//! Shared error type for MCP client + supervisor.

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("tool not found: {0}")]
    ToolNotFound(String),
    #[error("server returned error: {0}")]
    ServerError(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// Server requires OAuth/auth but the `TokenStore` had no bundle
    /// for `(server_id, tenant_id)`. The caller should run
    /// `xiaoguai mcp register --auth oauth2-pkce` (Tier-3 T4).
    #[error("authentication required: {0}")]
    AuthRequired(String),
    /// Token endpoint rejected the request (bad code, expired refresh,
    /// scope mismatch, etc.).
    #[error("authentication failed: {0}")]
    AuthFailed(String),
}

pub type McpResult<T> = Result<T, McpError>;
