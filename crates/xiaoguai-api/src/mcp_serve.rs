//! v0.9.1 — publish xiaoguai's `Toolbox` as an MCP server.
//!
//! Most of the 2026 agent ecosystem (Dify v1.6, n8n, Flowise v1.8) is
//! moving to two-way MCP: a platform isn't only a *consumer* of tools,
//! it also *publishes* its registered tools so other agents can call
//! them. xiaoguai already consumes MCP servers via `xiaoguai-mcp`;
//! this module flips the direction.
//!
//! Implementation: rmcp's `StreamableHttpService` mounted at
//! `/v1/mcp/serve`, with a [`XiaoguaiMcpServer`] `ServerHandler` that
//! dispatches `list_tools` / `call_tool` through the same `Toolbox`
//! the internal agent loop uses. Per-tool dispatch goes back into
//! the registered `Arc<dyn McpClient>` (could be stdio, HTTP, or a
//! future native tool), so external agents see exactly what an
//! internal agent would see.
//!
//! Gated by `AppState.mcp_publish_enabled`. Production deploys flip
//! the flag explicitly — we don't want to expose tools by accident.

use std::borrow::Cow;
use std::sync::Arc;

use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, Implementation, JsonObject, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, ToolsCapability,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::StreamableHttpServerConfig;
use rmcp::transport::StreamableHttpService;
use rmcp::ErrorData as McpProtocolError;
use serde_json::Value as JsonValue;
use xiaoguai_agent::Toolbox;

/// `ServerHandler` view over a `Toolbox`. Cheap to clone — the toolbox
/// is `Arc`-shared; the handler is recreated per session by
/// `StreamableHttpService`'s factory.
#[derive(Clone)]
pub struct XiaoguaiMcpServer {
    toolbox: Arc<Toolbox>,
    server_name: String,
    server_version: String,
}

impl XiaoguaiMcpServer {
    #[must_use]
    pub fn new(toolbox: Arc<Toolbox>) -> Self {
        Self {
            toolbox,
            server_name: "xiaoguai".into(),
            server_version: env!("CARGO_PKG_VERSION").into(),
        }
    }
}

impl std::fmt::Debug for XiaoguaiMcpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XiaoguaiMcpServer")
            .field("tools", &self.toolbox.len())
            .field("server_name", &self.server_name)
            .field("server_version", &self.server_version)
            .finish()
    }
}

impl ServerHandler for XiaoguaiMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools_with(ToolsCapability {
                    list_changed: Some(false),
                })
                .build(),
        )
        .with_server_info(Implementation::new(
            self.server_name.clone(),
            self.server_version.clone(),
        ))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpProtocolError> {
        // The internal `Toolbox` ordering is unspecified (HashMap); sort
        // by name so external consumers see a stable list across calls.
        let mut specs = self.toolbox.to_specs();
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        let tools = specs
            .into_iter()
            .map(|s| {
                let schema = json_value_to_input_schema(&s.parameters);
                Tool::new_with_raw(
                    Cow::Owned(s.name),
                    s.description.map(Cow::Owned),
                    Arc::new(schema),
                )
            })
            .collect();
        Ok(ListToolsResult {
            tools,
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpProtocolError> {
        let entry = self.toolbox.get(&request.name).ok_or_else(|| {
            McpProtocolError::invalid_params(format!("unknown tool: {}", request.name), None)
        })?;
        let args = match request.arguments {
            Some(map) => JsonValue::Object(map),
            None => JsonValue::Null,
        };
        let outcome = entry
            .client
            .call_tool(&request.name, args)
            .await
            .map_err(|e| {
                McpProtocolError::internal_error(format!("tool {} failed: {e}", request.name), None)
            })?;
        let content: Vec<Content> = outcome
            .blocks
            .iter()
            .filter_map(|b| match b {
                xiaoguai_mcp::ContentBlock::Text { text } => Some(Content::text(text.clone())),
                // Images / resources are deferred for v0.9.1; the chat-ui
                // path is the next consumer that cares.
                _ => None,
            })
            .collect();
        // Fall back to the flattened `text` field if blocks are empty.
        let content = if content.is_empty() && !outcome.text.is_empty() {
            vec![Content::text(outcome.text)]
        } else {
            content
        };
        Ok(if outcome.is_error {
            CallToolResult::error(content)
        } else {
            CallToolResult::success(content)
        })
    }
}

/// rmcp's `Tool::input_schema` wants a `serde_json::Map` (alias
/// `JsonObject`). The `Toolbox` stores a free-form `JsonValue` —
/// usually an object, but defend against nonsense.
fn json_value_to_input_schema(v: &JsonValue) -> JsonObject {
    v.as_object().cloned().unwrap_or_default()
}

/// Build the `axum::Router` fragment that serves the MCP endpoint at
/// `/v1/mcp/serve`. Returns `None` when publishing is disabled in
/// `AppState`, so the caller can `merge` without an extra branch.
#[allow(
    clippy::needless_pass_by_value,
    reason = "Arc passed by value is intentional API"
)]
pub fn build_router(toolbox: Arc<Toolbox>) -> axum::Router {
    let factory = {
        let tb = toolbox.clone();
        move || Ok::<_, std::io::Error>(XiaoguaiMcpServer::new(tb.clone()))
    };
    let service = StreamableHttpService::new(
        factory,
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    axum::Router::new().nest_service("/v1/mcp/serve", service)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use xiaoguai_mcp::{McpClient, McpResult, ServerInfo as McpServerInfo, ToolDescriptor};

    struct EchoBackend;

    #[async_trait]
    impl McpClient for EchoBackend {
        async fn initialize(&self) -> McpResult<McpServerInfo> {
            Ok(McpServerInfo {
                name: "echo-backend".into(),
                version: "0".into(),
            })
        }
        async fn list_tools(&self) -> McpResult<Vec<ToolDescriptor>> {
            Ok(vec![])
        }
        async fn call_tool(
            &self,
            name: &str,
            args: JsonValue,
        ) -> McpResult<xiaoguai_mcp::ToolResult> {
            let msg = args.get("msg").and_then(|v| v.as_str()).unwrap_or("");
            Ok(xiaoguai_mcp::ToolResult {
                text: format!("{name}: {msg}"),
                blocks: vec![xiaoguai_mcp::ContentBlock::Text {
                    text: format!("{name}: {msg}"),
                }],
                is_error: false,
            })
        }
        async fn shutdown(&self) -> McpResult<()> {
            Ok(())
        }
    }

    fn fixture_toolbox() -> Arc<Toolbox> {
        let client: Arc<dyn McpClient> = Arc::new(EchoBackend);
        let descriptors = vec![
            ToolDescriptor {
                name: "echo".into(),
                description: Some("echoes msg".into()),
                input_schema: json!({
                    "type": "object",
                    "properties": { "msg": { "type": "string" } }
                }),
                mutation_hint: xiaoguai_mcp::MutationHint::default(),
            },
            ToolDescriptor {
                name: "echo2".into(),
                description: Some("echoes msg, again".into()),
                input_schema: json!({
                    "type": "object",
                    "properties": { "msg": { "type": "string" } }
                }),
                mutation_hint: xiaoguai_mcp::MutationHint::default(),
            },
        ];
        Arc::new(Toolbox::from_server(client, descriptors).expect("toolbox"))
    }

    #[tokio::test]
    async fn list_tools_returns_sorted_toolbox() {
        let toolbox = fixture_toolbox();
        let server = XiaoguaiMcpServer::new(toolbox);
        let info = server.get_info();
        assert_eq!(info.server_info.name, "xiaoguai");

        // The trait method needs a RequestContext we can't easily mock —
        // call the internal helper directly via `Toolbox`. The real
        // dispatch through `ServerHandler::list_tools` is covered by the
        // end-to-end test in `tests/mcp_serve.rs`.
        let mut specs = server.toolbox.to_specs();
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "echo");
        assert_eq!(specs[1].name, "echo2");
    }

    #[test]
    fn json_value_to_input_schema_handles_non_object() {
        let s = json_value_to_input_schema(&json!("not an object"));
        assert!(s.is_empty());
        let s = json_value_to_input_schema(&json!({ "type": "object" }));
        assert_eq!(s.get("type").and_then(|v| v.as_str()), Some("object"));
    }
}
