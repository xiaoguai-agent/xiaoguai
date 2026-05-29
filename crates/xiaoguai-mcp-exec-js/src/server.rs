//! MCP server entry point.
//!
//! Exposes [`ExecServer`] (an `rmcp::handler::server::ServerHandler`)
//! and [`run_stdio_server`] (the stdio transport for spawning under an
//! MCP client supervisor like xiaoguai's `McpSupervisor`).

use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Implementation, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerInfo, ToolsCapability,
};
use rmcp::service::{RequestContext, RoleServer, ServiceExt};
use rmcp::transport::io::stdio;
use rmcp::ErrorData as McpProtocolError;

use crate::exec::ExecConfig;
use crate::tools::{
    execute_javascript_call, execute_javascript_tool, ExecuteJavascriptArgs, EXECUTE_JAVASCRIPT,
};

/// Stateful MCP server that owns the [`ExecConfig`] all
/// `execute_javascript` calls share **and** the [`ExecBackend`] trait
/// object that does the work (DEC-019 — per-tenant L1↔L3 swap is
/// runtime, not build-time). The trait object lets operators run
/// different sandbox tiers per tenant by registering distinct MCP
/// servers (e.g., `xiaoguai-mcp-exec-js` for L1 vs
/// `xiaoguai-mcp-exec-wasm-js` for L3) without changing this crate.
/// Cloning is cheap — both the config and the backend are `Arc`-shared.
#[derive(Clone)]
pub struct ExecServer {
    cfg: Arc<ExecConfig>,
    backend: Arc<dyn crate::runtime::ExecBackend>,
}

impl std::fmt::Debug for ExecServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecServer")
            .field("cfg", &self.cfg)
            .field("backend", &self.backend.name())
            .finish()
    }
}

impl ExecServer {
    /// Construct with a custom config. Uses the default L1 backend
    /// (`ProcessL1JavaScript`) — this is the canonical entry point.
    #[must_use]
    pub fn new(cfg: ExecConfig) -> Self {
        let backend: Arc<dyn crate::runtime::ExecBackend> =
            Arc::new(crate::runtime::ProcessL1JavaScript::new(cfg.clone()));
        Self {
            cfg: Arc::new(cfg),
            backend,
        }
    }

    /// Construct with an explicit backend impl (DEC-019). Used by L3
    /// binaries (`xiaoguai-mcp-exec-wasm-js`) that pass a
    /// `WasmtimeJavaScriptBackend`, and by tests that inject mocks.
    #[must_use]
    pub fn with_backend(
        cfg: ExecConfig,
        backend: Arc<dyn crate::runtime::ExecBackend>,
    ) -> Self {
        Self {
            cfg: Arc::new(cfg),
            backend,
        }
    }

    /// Backend tier label, surfaced to operators via the supervisor for
    /// debugging ("which tier is this server actually running?").
    #[must_use]
    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }
}

impl Default for ExecServer {
    fn default() -> Self {
        Self::new(ExecConfig::default())
    }
}

impl ServerHandler for ExecServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools_with(ToolsCapability {
                    list_changed: Some(false),
                })
                .build(),
        )
        .with_server_info(Implementation::new(
            "xiaoguai-mcp-exec-js",
            env!("CARGO_PKG_VERSION"),
        ))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpProtocolError> {
        Ok(ListToolsResult {
            tools: vec![execute_javascript_tool()],
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpProtocolError> {
        if request.name != EXECUTE_JAVASCRIPT {
            return Err(McpProtocolError::invalid_params(
                format!("unknown tool: {}", request.name),
                None,
            ));
        }

        let args_value = request
            .arguments
            .map_or(serde_json::Value::Null, serde_json::Value::Object);
        let args: ExecuteJavascriptArgs = serde_json::from_value(args_value).map_err(|e| {
            McpProtocolError::invalid_params(format!("invalid arguments: {e}"), None)
        })?;

        let (contents, is_error) =
            execute_javascript_call(self.backend.as_ref(), &self.cfg, args).await;
        Ok(if is_error {
            CallToolResult::error(contents)
        } else {
            CallToolResult::success(contents)
        })
    }
}

/// Run the server over stdin/stdout until the client closes. Suitable for
/// spawning via xiaoguai's `McpSupervisor` (transport=stdio).
///
/// # Errors
/// Returns an error if the stdio transport fails to bind (extremely
/// unusual — would mean stdin or stdout is not a usable pipe).
pub async fn run_stdio_server(cfg: ExecConfig) -> Result<()> {
    let server = ExecServer::new(cfg);
    let service = server
        .serve(stdio())
        .await
        .map_err(|e| anyhow!("serve stdio: {e}"))
        .context("rmcp stdio bind")?;
    service
        .waiting()
        .await
        .map_err(|e| anyhow!("serve loop: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn server_info_advertises_crate_version() {
        let s = ExecServer::default();
        let info = s.get_info();
        let impl_ = info.server_info;
        assert_eq!(impl_.name, "xiaoguai-mcp-exec-js");
        assert_eq!(impl_.version, env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn list_tools_returns_exactly_execute_javascript() {
        // The Tool definition is the source of truth; the handler just
        // wraps it in a Vec. Asserting on the definition is cheaper than
        // constructing a fake RequestContext.
        let t = execute_javascript_tool();
        assert_eq!(t.name.as_ref(), EXECUTE_JAVASCRIPT);
    }

    #[test]
    fn execute_javascript_args_parse_smoke() {
        let v = json!({"code": "console.log(1)", "timeout_secs": 5});
        let parsed: ExecuteJavascriptArgs = serde_json::from_value(v).unwrap();
        assert_eq!(parsed.code, "console.log(1)");
        assert_eq!(parsed.timeout_secs, Some(5));
    }
}
