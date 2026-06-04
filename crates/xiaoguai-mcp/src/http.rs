//! Streamable-HTTP MCP client built on rmcp's
//! `StreamableHttpClientTransport<reqwest::Client>`.
//!
//! v0.9.0 unblocks the ~80% of community MCP servers that don't ship a
//! stdio entry point: `OpenWebUI` tool servers, Continue's hosted
//! bundle, many marketplace entries on `LobeHub`, the official `GitHub`
//! MCP server, et al. The modern MCP spec ("Streamable HTTP") replaces
//! the original standalone-SSE transport with a single endpoint that
//! uses `POST` for requests and SSE for streaming responses; rmcp 1.7
//! consolidates both the spec-compliant flow and the SSE-streamed-
//! response legacy variants behind one transport, so we ship one
//! client here that covers both.
//!
//! Config surface mirrors what real MCP servers ask for:
//!   * `endpoint` — the server URL (e.g. `https://api.example.com/mcp`)
//!   * `auth_header` — bearer token / API key, passed verbatim in
//!     `Authorization`
//!   * `custom_headers` — for `X-Tenant-Id`, `X-Org`, etc. that some
//!     `SaaS` providers require
//!
//! The trait surface stays identical to [`StdioMcpClient`]; only the
//! constructor differs, so [`McpSupervisor`] / [`xiaoguai-api`] code
//! can swap transports by configuration without branching at call
//! sites.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientInfo, Implementation, RawContent,
    ResourceContents,
};
use rmcp::service::{RunningService, ServiceExt};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::RoleClient;
use serde_json::Value as JsonValue;

use crate::auth::oauth2_pkce::{
    build_http_client, refresh_pkce, should_refresh, OAuth2PkceConfig, TokenStore,
};
use crate::client::McpClient;
use crate::error::{McpError, McpResult};
use crate::types::{ContentBlock, ServerInfo, ToolDescriptor, ToolResult};

/// Builder-style configuration for [`HttpMcpClient`]. Mirrors the slice
/// of `StreamableHttpClientTransportConfig` we expose to operators —
/// hiding the rest behind sensible defaults (exponential-backoff retry,
/// 16-message channel, `allow_stateless = true`, reinit on session
/// expiry).
#[derive(Debug, Clone)]
pub struct HttpClientConfig {
    pub endpoint: String,
    /// Sent as `Authorization: <value>`. Most MCP `SaaS` providers want
    /// `Bearer <token>` — pass the full string including the scheme.
    pub auth_header: Option<String>,
    /// Extra headers (`X-Tenant-Id`, `X-Org`, …). Keys are case-sensitive
    /// on the wire per HTTP/2.
    pub custom_headers: Vec<(String, String)>,
}

impl HttpClientConfig {
    #[must_use]
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            auth_header: None,
            custom_headers: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_auth(mut self, header_value: impl Into<String>) -> Self {
        self.auth_header = Some(header_value.into());
        self
    }

    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.custom_headers.push((name.into(), value.into()));
        self
    }
}

pub struct HttpMcpClient {
    service: RunningService<RoleClient, ClientInfo>,
}

impl std::fmt::Debug for HttpMcpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpMcpClient")
            .field(
                "peer",
                &self.service.peer_info().map(|p| &p.server_info.name),
            )
            .finish()
    }
}

impl HttpMcpClient {
    /// Connect to an MCP server over Streamable HTTP, using a
    /// `TokenStore` to resolve the OAuth bearer token. Refreshes the
    /// stored bundle if it expires within
    /// [`crate::auth::REFRESH_LEEWAY_SECS`] seconds.
    ///
    /// Tier-3 T4 entry point. If the store has no bundle for
    /// `server_id` and `cfg.auth_header` is also unset, returns
    /// [`McpError::AuthRequired`] so the caller can prompt the
    /// operator to register the server.
    ///
    /// # Errors
    /// Returns [`McpError::AuthRequired`] if no token is on file and
    /// no static `auth_header` was supplied; any error returned by
    /// the underlying refresh or `connect` paths.
    pub async fn connect_with_store(
        mut cfg: HttpClientConfig,
        store: Arc<dyn TokenStore>,
        oauth_cfg: &OAuth2PkceConfig,
        server_id: &str,
    ) -> McpResult<Self> {
        let existing = store.get(server_id).await?;
        let bundle = match existing {
            Some(b) if should_refresh(&b, chrono::Utc::now()) => {
                let http = build_http_client()?;
                let refreshed = refresh_pkce(&http, oauth_cfg, &b).await?;
                store.put(server_id, &refreshed).await?;
                refreshed
            }
            Some(b) => b,
            None => {
                if cfg.auth_header.is_none() {
                    return Err(McpError::AuthRequired(format!(
                        "no token bundle for server_id={server_id}; \
                         run `xiaoguai mcp register --auth oauth2-pkce ...`"
                    )));
                }
                return Self::connect(cfg).await;
            }
        };
        cfg.auth_header = Some(format!("Bearer {}", bundle.access_token));
        Self::connect(cfg).await
    }

    /// Connect to an MCP server over Streamable HTTP and complete the
    /// `initialize` handshake.
    ///
    /// # Errors
    /// Returns an error if the connection or initialization fails.
    pub async fn connect(cfg: HttpClientConfig) -> McpResult<Self> {
        let mut transport_cfg = StreamableHttpClientTransportConfig::with_uri(cfg.endpoint);
        if let Some(value) = cfg.auth_header {
            transport_cfg = transport_cfg.auth_header(value);
        }
        if !cfg.custom_headers.is_empty() {
            let mut hm = HashMap::with_capacity(cfg.custom_headers.len());
            for (k, v) in cfg.custom_headers {
                let name = http::HeaderName::try_from(k.as_str())
                    .map_err(|e| McpError::InvalidArgument(format!("header name {k:?}: {e}")))?;
                let value = http::HeaderValue::try_from(v.as_str())
                    .map_err(|e| McpError::InvalidArgument(format!("header value for {k}: {e}")))?;
                hm.insert(name, value);
            }
            transport_cfg = transport_cfg.custom_headers(hm);
        }
        let transport = StreamableHttpClientTransport::from_config(transport_cfg);

        let client_info = ClientInfo::new(
            ClientCapabilities::default(),
            Implementation::new("xiaoguai", env!("CARGO_PKG_VERSION")),
        );
        let service = client_info
            .serve(transport)
            .await
            .map_err(|e| McpError::Protocol(format!("initialize: {e}")))?;
        Ok(Self { service })
    }
}

fn resource_uri(rc: &ResourceContents) -> String {
    match rc {
        ResourceContents::TextResourceContents { uri, .. }
        | ResourceContents::BlobResourceContents { uri, .. } => uri.clone(),
    }
}

#[async_trait]
impl McpClient for HttpMcpClient {
    async fn initialize(&self) -> McpResult<ServerInfo> {
        let info = self
            .service
            .peer_info()
            .ok_or_else(|| McpError::Protocol("peer info not populated post-handshake".into()))?;
        Ok(ServerInfo {
            name: info.server_info.name.clone(),
            version: info.server_info.version.clone(),
        })
    }

    async fn list_tools(&self) -> McpResult<Vec<ToolDescriptor>> {
        let resp = self
            .service
            .list_tools(Option::default())
            .await
            .map_err(|e| McpError::Protocol(format!("list_tools: {e}")))?;
        Ok(resp
            .tools
            .into_iter()
            .map(|t| ToolDescriptor {
                name: t.name.into_owned(),
                description: t.description.map(std::borrow::Cow::into_owned),
                input_schema: serde_json::to_value(&*t.input_schema).unwrap_or(JsonValue::Null),
            })
            .collect())
    }

    async fn call_tool(&self, name: &str, args: JsonValue) -> McpResult<ToolResult> {
        let arguments = match args {
            JsonValue::Null => None,
            JsonValue::Object(map) => Some(map),
            other => {
                return Err(McpError::InvalidArgument(format!(
                    "tool arguments must be a JSON object or null, got: {other}"
                )));
            }
        };
        let mut params = CallToolRequestParams::new(name.to_string());
        params.arguments = arguments;
        let resp = self
            .service
            .call_tool(params)
            .await
            .map_err(|e| McpError::Protocol(format!("call_tool: {e}")))?;

        let is_error = resp.is_error.unwrap_or(false);
        let mut text = String::new();
        let mut blocks: Vec<ContentBlock> = Vec::with_capacity(resp.content.len());
        for c in resp.content {
            match c.raw {
                RawContent::Text(t) => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&t.text);
                    blocks.push(ContentBlock::Text { text: t.text });
                }
                RawContent::Image(i) => {
                    blocks.push(ContentBlock::Image {
                        mime_type: i.mime_type,
                        data_base64: i.data,
                    });
                }
                RawContent::Resource(r) => {
                    blocks.push(ContentBlock::Resource {
                        uri: resource_uri(&r.resource),
                    });
                }
                // Audio + ResourceLink: same policy as the stdio impl —
                // drop until the agent loop knows what to do with them.
                _ => {}
            }
        }
        Ok(ToolResult {
            text,
            blocks,
            is_error,
        })
    }

    async fn shutdown(&self) -> McpResult<()> {
        // `RunningService` cancels its task on Drop — identical contract
        // to `StdioMcpClient`. Callers drop the `Arc<dyn McpClient>` to
        // release resources.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_builder_round_trips() {
        let cfg = HttpClientConfig::new("https://example.invalid/mcp")
            .with_auth("Bearer abc.def")
            .with_header("X-Tenant", "ten_a")
            .with_header("X-Trace", "t-1");
        assert_eq!(cfg.endpoint, "https://example.invalid/mcp");
        assert_eq!(cfg.auth_header.as_deref(), Some("Bearer abc.def"));
        assert_eq!(cfg.custom_headers.len(), 2);
        assert_eq!(cfg.custom_headers[0], ("X-Tenant".into(), "ten_a".into()));
    }

    #[test]
    fn header_validation_rejects_malformed_names() {
        // CRLF in a header name must be refused, not passed to reqwest.
        // We can't easily test this through `connect` without a server;
        // instead, exercise the same conversion path so changes to the
        // validation policy stay obvious.
        let bad = "X-Foo\r\nInjected: yes";
        assert!(http::HeaderName::try_from(bad).is_err());
        let good = "X-Tenant";
        assert!(http::HeaderName::try_from(good).is_ok());
    }
}
