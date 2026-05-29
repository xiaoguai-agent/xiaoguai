#![allow(clippy::doc_markdown)]
//! `xiaoguai-mcp-exec-wasm-js` — L3 JavaScript MCP stdio server binary.
//!
//! Mirrors the Python L3 binary; uses `WasmtimeJavaScriptBackend` and
//! the `execute_javascript` MCP tool name (matches the L1 JS server's
//! tool name so clients flipping tiers don't need to rename).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
    ToolsCapability,
};
use rmcp::service::{RequestContext, RoleServer, ServiceExt};
use rmcp::transport::io::stdio;
use rmcp::ErrorData as McpProtocolError;
use serde::Deserialize;
use serde_json::json;
use xiaoguai_mcp_exec::runtime::ExecBackend;
use xiaoguai_mcp_exec::{ExecError, ExecResult};
use xiaoguai_mcp_exec_wasm::{WasmExecConfig, WasmtimeJavaScriptBackend};

const EXECUTE_JAVASCRIPT: &str = "execute_javascript";
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 60;

#[derive(Parser, Debug)]
#[command(
    name = "xiaoguai-mcp-exec-wasm-js",
    version,
    about = "L3 sandboxed JavaScript execution MCP server (wasmtime + QuickJS-WASM)"
)]
struct Cli {
    #[arg(long, env = "XIAOGUAI_MCP_EXEC_WASM__TIMEOUT_SECS", default_value_t = 30)]
    timeout_secs: u64,

    #[arg(long, env = "XIAOGUAI_MCP_EXEC_WASM__MEMORY_MB", default_value_t = 256)]
    memory_mb: u64,

    /// Override the QuickJS WASM module path. Defaults to reading
    /// `XIAOGUAI_QUICKJS_PATH` at request time.
    #[arg(long, env = "XIAOGUAI_QUICKJS_PATH")]
    quickjs_path: Option<PathBuf>,

    #[arg(long, env = "XIAOGUAI_MCP_EXEC_WASM__NO_REDACT")]
    no_redact_stderr: bool,
}

#[derive(Debug, Deserialize)]
struct ExecuteArgs {
    code: String,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

#[derive(Clone)]
struct Server {
    backend: Arc<WasmtimeJavaScriptBackend>,
}

impl Server {
    fn new(cfg: WasmExecConfig) -> Self {
        Self {
            backend: Arc::new(WasmtimeJavaScriptBackend::new(cfg)),
        }
    }
}

fn execute_javascript_tool() -> Tool {
    let schema = json!({
        "type": "object",
        "properties": {
            "code": {"type": "string"},
            "timeout_secs": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_TIMEOUT_SECS,
                "default": DEFAULT_TIMEOUT_SECS
            }
        },
        "required": ["code"],
        "additionalProperties": false
    });
    let schema_obj = Arc::new(schema.as_object().cloned().unwrap_or_default());
    let annotations = ToolAnnotations::new()
        .read_only(false)
        .destructive(false)
        .idempotent(false)
        .open_world(false);
    Tool::new(
        EXECUTE_JAVASCRIPT,
        "[WRITE] Execute a self-contained JavaScript snippet in an L3 wasmtime + QuickJS sandbox. Capability-based isolation: no host syscalls, no fs, no network, no env. Cold start ~50-200 ms; per-call memory cap default 256 MB.",
        schema_obj,
    )
    .annotate(annotations)
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools_with(ToolsCapability {
                    list_changed: Some(false),
                })
                .build(),
        )
        .with_server_info(Implementation::new(
            "xiaoguai-mcp-exec-wasm-js",
            env!("CARGO_PKG_VERSION"),
        ))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpProtocolError> {
        Ok(ListToolsResult {
            tools: vec![execute_javascript_tool()],
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _ctx: RequestContext<RoleServer>,
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
        let args: ExecuteArgs = serde_json::from_value(args_value).map_err(|e| {
            McpProtocolError::invalid_params(format!("invalid arguments: {e}"), None)
        })?;
        let timeout = Duration::from_secs(
            args.timeout_secs
                .unwrap_or(DEFAULT_TIMEOUT_SECS)
                .min(MAX_TIMEOUT_SECS),
        );

        let (contents, is_error) = match self.backend.run(&args.code, timeout).await {
            Ok(r) => exec_result_to_content(&r),
            Err(ExecError::SnippetTooLarge(n)) => (
                vec![Content::text(format!(
                    "snippet is {n} bytes; max 65536. Trim it or split into multiple calls."
                ))],
                true,
            ),
            Err(other) => (
                vec![Content::text(format!("L3 supervisor error: {other}"))],
                true,
            ),
        };
        Ok(if is_error {
            CallToolResult::error(contents)
        } else {
            CallToolResult::success(contents)
        })
    }
}

fn exec_result_to_content(r: &ExecResult) -> (Vec<Content>, bool) {
    let payload = json!({
        "exit_code": r.exit_code,
        "stdout": r.stdout,
        "stderr": r.stderr,
        "duration_ms": r.duration_ms,
        "truncated": r.truncated,
        "timed_out": r.timed_out,
    });
    let text =
        serde_json::to_string(&payload).unwrap_or_else(|e| format!(r#"{{"error":"{e}"}}"#));
    (vec![Content::text(text)], false)
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    let cli = Cli::parse();
    let cfg = WasmExecConfig {
        max_timeout: Duration::from_secs(cli.timeout_secs),
        memory_mb: cli.memory_mb,
        redact_stderr: !cli.no_redact_stderr,
        module_path: cli.quickjs_path,
    };
    tracing::info!(
        timeout_secs = cli.timeout_secs,
        memory_mb = cli.memory_mb,
        redact_stderr = cfg.redact_stderr,
        "xiaoguai-mcp-exec-wasm-js: starting stdio server"
    );

    let server = Server::new(cfg);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
