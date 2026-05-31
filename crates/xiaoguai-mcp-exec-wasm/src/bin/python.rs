#![allow(clippy::doc_markdown)]
//! `xiaoguai-mcp-exec-wasm-py` — L3 Python MCP stdio server binary.
//!
//! Per DEC-020: separate binary from the L1 Python server so operators
//! can deploy only the L3 surface when they want capability isolation.
//! The MCP tool name is still `execute_python` so client configs that
//! switch tiers don't need to rename tools.
//!
//! S8-4 (per-tenant tier selector) is out of scope for this PR — for
//! now the binary is a thin runner around the L3 backend with no
//! L1 fallback path. Operators who want both should deploy two
//! servers.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, ToolAnnotations, ToolsCapability,
};
use rmcp::service::{RequestContext, RoleServer, ServiceExt};
use rmcp::transport::io::stdio;
use rmcp::ErrorData as McpProtocolError;
use serde::Deserialize;
use serde_json::json;
use xiaoguai_mcp_exec::runtime::ExecBackend;
use xiaoguai_mcp_exec::{ExecError, ExecResult};
use xiaoguai_mcp_exec_wasm::{WasmExecConfig, WasmtimePythonBackend};

const EXECUTE_PYTHON: &str = "execute_python";
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 60;

#[derive(Parser, Debug)]
#[command(
    name = "xiaoguai-mcp-exec-wasm-py",
    version,
    about = "L3 sandboxed Python execution MCP server (wasmtime + pyodide)"
)]
struct Cli {
    /// Hard wall-clock cap per call (seconds).
    #[arg(
        long,
        env = "XIAOGUAI_MCP_EXEC_WASM__TIMEOUT_SECS",
        default_value_t = 30
    )]
    timeout_secs: u64,

    /// Memory cap (megabytes). Default 256 — pyodide baseline is ~30 MB.
    #[arg(long, env = "XIAOGUAI_MCP_EXEC_WASM__MEMORY_MB", default_value_t = 256)]
    memory_mb: u64,

    /// Override the pyodide WASM module path. Defaults to reading
    /// `XIAOGUAI_PYODIDE_PATH` at request time.
    #[arg(long, env = "XIAOGUAI_PYODIDE_PATH")]
    pyodide_path: Option<PathBuf>,

    /// Disable stderr PII redaction.
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
    backend: Arc<WasmtimePythonBackend>,
}

impl Server {
    fn new(cfg: WasmExecConfig) -> Self {
        Self {
            backend: Arc::new(WasmtimePythonBackend::new(cfg)),
        }
    }
}

fn execute_python_tool() -> Tool {
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
        EXECUTE_PYTHON,
        "[WRITE] Execute a self-contained Python 3 snippet in an L3 wasmtime + pyodide sandbox. Capability-based isolation: no host syscalls, no fs, no network, no env. Cold start ~50-200 ms; per-call memory cap default 256 MB.",
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
            "xiaoguai-mcp-exec-wasm-py",
            env!("CARGO_PKG_VERSION"),
        ))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpProtocolError> {
        Ok(ListToolsResult {
            tools: vec![execute_python_tool()],
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpProtocolError> {
        if request.name != EXECUTE_PYTHON {
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
    let text = serde_json::to_string(&payload).unwrap_or_else(|e| format!(r#"{{"error":"{e}"}}"#));
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
        module_path: cli.pyodide_path,
    };
    tracing::info!(
        timeout_secs = cli.timeout_secs,
        memory_mb = cli.memory_mb,
        redact_stderr = cfg.redact_stderr,
        "xiaoguai-mcp-exec-wasm-py: starting stdio server"
    );

    let server = Server::new(cfg);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
