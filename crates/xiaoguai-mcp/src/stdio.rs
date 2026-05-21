//! Stdio-transport MCP client built on rmcp's `TokioChildProcess` transport.

use std::ffi::OsStr;
use std::path::Path;

use async_trait::async_trait;
use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientInfo, Implementation, RawContent,
    ResourceContents,
};
use rmcp::service::{RunningService, ServiceExt};
use rmcp::transport::TokioChildProcess;
use rmcp::RoleClient;
use serde_json::Value as JsonValue;
use tokio::process::Command;

use crate::client::McpClient;
use crate::error::{McpError, McpResult};
use crate::types::{ContentBlock, ServerInfo, ToolDescriptor, ToolResult};

pub struct StdioMcpClient {
    service: RunningService<RoleClient, ClientInfo>,
}

impl std::fmt::Debug for StdioMcpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdioMcpClient")
            .field(
                "peer",
                &self.service.peer_info().map(|p| &p.server_info.name),
            )
            .finish()
    }
}

impl StdioMcpClient {
    /// Spawn the given binary and complete the MCP `initialize` handshake.
    ///
    /// `args` are passed positional after the binary path. `envs` are extra
    /// environment variables for the child process (inherits the parent's
    /// `PATH` etc. by default).
    pub async fn spawn<P, S, T>(program: P, args: &[&str], envs: &[(S, T)]) -> McpResult<Self>
    where
        P: AsRef<Path>,
        S: AsRef<OsStr>,
        T: AsRef<OsStr>,
    {
        let mut cmd = Command::new(program.as_ref());
        cmd.args(args);
        for (k, v) in envs {
            cmd.env(k.as_ref(), v.as_ref());
        }
        let transport =
            TokioChildProcess::new(cmd).map_err(|e| McpError::Transport(format!("spawn: {e}")))?;

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
impl McpClient for StdioMcpClient {
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
        // rmcp accepts the arguments as JsonObject (Map<String, Value>) or
        // None. Reject non-object inputs early with a clear error.
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
                // Audio + ResourceLink: surfacing these blocks lands with the
                // ReAct loop in v0.5.4 once we know what the agent actually
                // does with them. Drop silently for now.
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
        // `RunningService` cancels its task and reaps the child process on
        // Drop. An explicit cancel would consume `self`, so we rely on Drop —
        // the caller dropping the client (or its containing `Arc`) is the
        // documented contract.
        Ok(())
    }
}
