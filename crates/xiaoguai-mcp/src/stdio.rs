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

/// Env vars passed through from the parent to MCP child processes.
/// Everything else is scrubbed (SEC-03) so host secrets — audit signing
/// key, AWS creds, IM tokens, provider API keys — are never handed to
/// third-party MCP servers. Just enough for the child runtime to find
/// binaries, its home/temp dirs, and decode paths/messages. Mirrors the
/// allowlists in `xiaoguai-coding/src/git.rs` and `xiaoguai-mcp-exec`.
const ALLOWED_PASSTHROUGH: &[&str] = &[
    "PATH", "HOME",   // many runtimes resolve config/cache under $HOME
    "TMPDIR", // writable temp dir (node/python/uv need one)
    "LANG", "LC_ALL", "LC_CTYPE", // locale
];

/// Fallback `PATH` when the parent has none, so `npx`/`uvx`-style
/// launchers stay resolvable after the SEC-03 scrub.
const DEFAULT_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

pub struct StdioMcpClient {
    service: RunningService<RoleClient, ClientInfo>,
    /// #286: whether this server's self-declared `readOnlyHint` is honored
    /// when classifying tools (`MutationHint`). `false` (the default) maps
    /// every tool to `Write` so consult mode excludes it — an external
    /// server's word alone is not a read-only guarantee. Operators opt a
    /// server in via [`Self::with_trusted_read_only_hints`].
    trust_read_only_hints: bool,
}

impl std::fmt::Debug for StdioMcpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdioMcpClient")
            .field(
                "peer",
                // rmcp 1.8: peer_info() returns an owned Option<ServerInfo>, so
                // clone the name rather than borrow the closure-local value.
                &self.service.peer_info().map(|p| p.server_info.name.clone()),
            )
            .finish_non_exhaustive()
    }
}

impl StdioMcpClient {
    /// Spawn the given binary and complete the MCP `initialize` handshake.
    ///
    /// `args` are passed positional after the binary path. The child's
    /// environment is scrubbed (SEC-03): only [`ALLOWED_PASSTHROUGH`] vars
    /// are inherited from the parent, then the caller's explicit `envs` are
    /// applied on top (they win over the passthrough set).
    ///
    /// # Errors
    /// Returns `McpError::Transport` if the child process cannot be spawned,
    /// or `McpError::Protocol` if the MCP `initialize` handshake fails.
    pub async fn spawn<P, S, T>(program: P, args: &[&str], envs: &[(S, T)]) -> McpResult<Self>
    where
        P: AsRef<Path>,
        S: AsRef<OsStr>,
        T: AsRef<OsStr>,
    {
        let mut cmd = Command::new(program.as_ref());
        cmd.args(args);
        // SEC-03: never let third-party MCP servers inherit the host's full
        // environment (audit signing key, AWS creds, IM tokens, …). Clear
        // everything, re-add the minimal allowlist, then the caller's
        // explicit `envs` last so they override the passthrough set.
        cmd.env_clear();
        for key in ALLOWED_PASSTHROUGH {
            if let Ok(val) = std::env::var(key) {
                cmd.env(key, val);
            }
        }
        if std::env::var("PATH").is_err() {
            cmd.env("PATH", DEFAULT_PATH);
        }
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
        Ok(Self {
            service,
            // #286: distrust by default — see `with_trusted_read_only_hints`.
            trust_read_only_hints: false,
        })
    }

    /// #286: opt this server into having its self-declared `readOnlyHint`
    /// honored. Only set when the operator explicitly trusted the server
    /// (see `supervisor`'s `XIAOGUAI_MCP_TRUST_READ_ONLY_HINTS` allowlist) —
    /// a trusted server's `readOnlyHint: true` tools become consult-eligible.
    #[must_use]
    pub fn with_trusted_read_only_hints(mut self, trust: bool) -> Self {
        self.trust_read_only_hints = trust;
        self
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
            // #286: `readOnlyHint` only survives for operator-trusted servers.
            .map(|t| {
                crate::rmcp_convert::descriptor_from_rmcp_tool(t, self.trust_read_only_hints)
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
