//! MCP server registry domain type.
//!
//! Mirrors the `mcp_servers` Postgres table. The `env_keys` field intentionally
//! stores env-variable **names**, not values; the runtime resolves them at
//! spawn time (same secrets policy as `LlmProvider::api_key_env`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::{McpServerInstanceId, TenantId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpTransport {
    #[serde(rename = "stdio")]
    Stdio,
    #[serde(rename = "sse")]
    Sse,
    #[serde(rename = "http")]
    Http,
}

impl McpTransport {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Sse => "sse",
            Self::Http => "http",
        }
    }

    /// Parse the DB string. Named `parse` to avoid clashing with `FromStr`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "stdio" => Some(Self::Stdio),
            "sse" => Some(Self::Sse),
            "http" => Some(Self::Http),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServer {
    pub id: McpServerInstanceId,
    /// `None` = system-wide, visible to every tenant.
    pub tenant_id: Option<TenantId>,
    pub name: String,
    pub version: String,
    pub transport: McpTransport,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env_keys: Vec<String>,
    pub endpoint: Option<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_round_trips() {
        for t in [McpTransport::Stdio, McpTransport::Sse, McpTransport::Http] {
            assert_eq!(McpTransport::parse(t.as_str()), Some(t));
        }
    }

    #[test]
    fn unknown_transport_returns_none() {
        assert_eq!(McpTransport::parse("websocket"), None);
    }
}
