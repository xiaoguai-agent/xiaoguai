//! `GET /v1/mcp/servers` — list MCP servers visible to the caller.

use axum::extract::{Extension, State};
use axum::Json;
use serde::Serialize;
use xiaoguai_types::{McpServer, McpTransport};

use crate::auth::Claims;
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
    pub tenant_id: Option<String>,
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
            tenant_id: s.tenant_id.map(|t| t.to_string()),
        }
    }
}

/// # Errors
/// Returns an error if the MCP server registry is not wired or the query fails.
pub async fn list_servers(
    State(state): State<AppState>,
    claims: Option<Extension<Claims>>,
) -> ApiResult<Json<Vec<McpServerResponse>>> {
    let repo = state.mcp_servers.as_ref().ok_or_else(|| {
        ApiError::Internal(anyhow::anyhow!(
            "MCP server registry not wired into AppState"
        ))
    })?;

    // If a tenant is in scope, return globals + tenant-scoped rows;
    // otherwise globals only. v0.6 contract: claims tenant_id wins;
    // anonymous callers see globals.
    let rows = if let Some(Extension(c)) = claims {
        repo.list_for_tenant(&c.tenant_id).await?
    } else {
        repo.list_global().await?
    };

    Ok(Json(rows.into_iter().map(Into::into).collect()))
}
