//! `GET /v1/mcp/servers` — list MCP servers visible to the caller.

use axum::extract::State;
use axum::Json;
use serde::Serialize;
use xiaoguai_types::{McpServer, McpTransport};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct McpServerResponse {
    pub id: String,
    pub name: String,
    pub version: String,
    pub transport: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env_keys: Vec<String>,
    pub endpoint: Option<String>,
}

impl From<McpServer> for McpServerResponse {
    fn from(s: McpServer) -> Self {
        Self {
            id: s.id.to_string(),
            name: s.name,
            version: s.version,
            transport: match s.transport {
                McpTransport::Stdio => "stdio",
                McpTransport::Sse => "sse",
                McpTransport::Http => "http",
            }
            .to_string(),
            command: s.command,
            args: s.args,
            env_keys: s.env_keys,
            endpoint: s.endpoint,
        }
    }
}

/// # Errors
/// Returns an error if the MCP server registry is not wired or the query fails.
pub async fn list_servers(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<McpServerResponse>>> {
    let repo = state.mcp_servers.as_ref().ok_or_else(|| {
        ApiError::Internal(anyhow::anyhow!(
            "MCP server registry not wired into AppState"
        ))
    })?;

    let rows = repo.list().await?;

    Ok(Json(rows.into_iter().map(Into::into).collect()))
}
