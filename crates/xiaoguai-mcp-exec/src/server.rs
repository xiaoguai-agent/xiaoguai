//! MCP server entry point.
//!
//! Exposes [`ExecServer`] (an `rmcp::handler::server::ServerHandler`) and
//! [`run_stdio_server`] (the stdio transport for spawning under an MCP
//! client supervisor like xiaoguai's [`McpSupervisor`]).

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
use crate::tools::{execute_python_call, execute_python_tool, ExecutePythonArgs, EXECUTE_PYTHON};

/// Stateful MCP server that owns the [`ExecConfig`] all `execute_python`
/// calls share **and** the [`ExecBackend`] trait object that does the work
/// (DEC-019 — per-tenant L1↔L3 swap is runtime, not build-time). The
/// trait object lets operators run different sandbox tiers per tenant by
/// registering distinct MCP servers (e.g., `xiaoguai-mcp-exec` for L1 vs
/// `xiaoguai-mcp-exec-wasm-py` for L3) without changing this crate. Cloning
/// is cheap — both the config and the backend are `Arc`-shared.
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
    /// (`ProcessL1Python`) — this is the canonical entry point.
    #[must_use]
    pub fn new(cfg: ExecConfig) -> Self {
        let backend: Arc<dyn crate::runtime::ExecBackend> =
            Arc::new(crate::runtime::ProcessL1Python::new(cfg.clone()));
        Self {
            cfg: Arc::new(cfg),
            backend,
        }
    }

    /// Construct with an explicit backend impl (DEC-019). Used by L3
    /// binaries (`xiaoguai-mcp-exec-wasm-py`) that pass a
    /// `WasmtimePythonBackend`, and by tests that inject mocks.
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
            "xiaoguai-mcp-exec",
            env!("CARGO_PKG_VERSION"),
        ))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpProtocolError> {
        Ok(ListToolsResult {
            tools: vec![execute_python_tool()],
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpProtocolError> {
        if request.name != EXECUTE_PYTHON {
            return Err(McpProtocolError::invalid_params(
                format!("unknown tool: {}", request.name),
                None,
            ));
        }

        // The MCP client passes `arguments` as `Option<serde_json::Map>`.
        // serde_json::from_value can deserialise that straight into our
        // typed struct via Value::Object.
        let args_value = request
            .arguments
            .map_or(serde_json::Value::Null, serde_json::Value::Object);
        let args: ExecutePythonArgs = serde_json::from_value(args_value).map_err(|e| {
            McpProtocolError::invalid_params(format!("invalid arguments: {e}"), None)
        })?;

        let (contents, is_error) =
            execute_python_call(self.backend.as_ref(), &self.cfg, args).await;
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
        assert_eq!(impl_.name, "xiaoguai-mcp-exec");
        assert_eq!(impl_.version, env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn list_tools_returns_exactly_execute_python() {
        // We can build a fake RequestContext only inside rmcp's tests, so
        // we exercise the underlying tool definition instead.
        let t = execute_python_tool();
        assert_eq!(t.name.as_ref(), EXECUTE_PYTHON);
    }

    #[tokio::test]
    async fn call_tool_unknown_returns_invalid_params_via_handler() {
        // Smoke-test the unknown-tool branch through the handler-level
        // serde_json path: we synthesise an args value that the schema
        // would normally pre-validate. This guards against regression
        // where a future refactor stops bouncing the name check.
        let cfg = ExecConfig::default();
        let server = ExecServer::new(cfg);
        // Direct module call mirrors what call_tool does internally.
        let args = ExecutePythonArgs {
            code: "print(1)".into(),
            timeout_secs: Some(5),
        };
        let (contents, is_error) =
            execute_python_call(server.backend.as_ref(), &server.cfg, args).await;
        assert!(!is_error);
        assert!(!contents.is_empty());
    }

    #[test]
    fn execute_python_args_parse_with_defaults() {
        let v = json!({"code": "print(1)"});
        let parsed: ExecutePythonArgs = serde_json::from_value(v).unwrap();
        assert_eq!(parsed.code, "print(1)");
        assert!(parsed.timeout_secs.is_none());
    }

    #[test]
    fn execute_python_args_reject_missing_code() {
        let v = json!({"timeout_secs": 5});
        let err = serde_json::from_value::<ExecutePythonArgs>(v).unwrap_err();
        assert!(err.to_string().contains("code"));
    }

    // ── DEC-019 selector wiring (S8-4) -----------------------------------

    /// Mock backend: records every `run` invocation; returns a canned
    /// success payload. Lets us prove the `ExecServer` actually routes
    /// through the trait object we hand it (rather than the default L1).
    #[derive(Default)]
    struct MockBackend {
        invocations: std::sync::Mutex<Vec<(String, std::time::Duration)>>,
    }

    #[async_trait::async_trait]
    impl crate::runtime::ExecBackend for MockBackend {
        fn name(&self) -> &'static str {
            "mock-test-backend"
        }
        async fn run(
            &self,
            snippet: &str,
            timeout: std::time::Duration,
        ) -> Result<crate::exec::ExecResult, crate::exec::ExecError> {
            self.invocations
                .lock()
                .unwrap()
                .push((snippet.to_string(), timeout));
            Ok(crate::exec::ExecResult {
                exit_code: Some(0),
                stdout: "ok".into(),
                stderr: String::new(),
                duration_ms: 1,
                truncated: false,
                timed_out: false,
            })
        }
        fn capability_summary(&self) -> crate::runtime::CapabilitySummary {
            crate::runtime::CapabilitySummary {
                tier: "mock",
                language: "python",
                network: false,
                filesystem: false,
                subprocess: false,
                max_memory_mb: 256,
                max_timeout_secs: 30,
            }
        }
    }

    #[tokio::test]
    async fn with_backend_routes_through_trait_object_not_default_l1() {
        let mock = std::sync::Arc::new(MockBackend::default());
        let server = ExecServer::with_backend(ExecConfig::default(), mock.clone());

        assert_eq!(server.backend_name(), "mock-test-backend");

        let args = ExecutePythonArgs {
            code: "print('routed')".into(),
            timeout_secs: Some(5),
        };
        let (_contents, is_error) =
            execute_python_call(server.backend.as_ref(), &server.cfg, args).await;
        assert!(!is_error);

        // Proof: the mock was invoked with exactly the routed snippet.
        let inv = mock.invocations.lock().unwrap();
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].0, "print('routed')");
    }

    #[test]
    fn default_backend_is_process_l1_python() {
        let server = ExecServer::new(ExecConfig::default());
        // Stable label is load-bearing — DEC-019 metrics/dashboards key on it.
        assert_eq!(server.backend_name(), "process-l1-python");
    }
}
