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
}

pub type McpResult<T> = Result<T, McpError>;
