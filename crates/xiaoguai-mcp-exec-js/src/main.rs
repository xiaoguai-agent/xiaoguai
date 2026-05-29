//! `xiaoguai-mcp-exec-js` binary — stdio MCP transport for the JS sandbox.
//!
//! Reads MCP protocol from stdin, writes results to stdout. Intended to
//! be spawned by an MCP supervisor (xiaoguai's `McpSupervisor` or any
//! other compliant client) with `transport=stdio`.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use xiaoguai_mcp_exec_js::{run_stdio_server, ExecConfig, Runtime};

#[derive(Parser, Debug)]
#[command(
    name = "xiaoguai-mcp-exec-js",
    version,
    about = "Sandboxed JavaScript code-execution MCP server (Deno or Node)"
)]
struct Cli {
    /// Hard wall-clock cap per call (seconds). Per-call timeouts above
    /// this are clamped.
    #[arg(long, env = "XIAOGUAI_MCP_EXEC_JS__TIMEOUT_SECS", default_value_t = 30)]
    timeout_secs: u64,

    /// Address-space limit (megabytes) per call. Passed to `ulimit -v`.
    /// V8 reserves a larger up-front heap than `CPython`; default 1024.
    #[arg(long, env = "XIAOGUAI_MCP_EXEC_JS__MEMORY_MB", default_value_t = 1024)]
    memory_mb: u64,

    /// Parent directory for per-call tempdirs. Defaults to the OS temp dir.
    #[arg(long, env = "XIAOGUAI_MCP_EXEC_JS__WORKDIR_PARENT")]
    workdir_parent: Option<PathBuf>,

    /// JS runtime: `deno` (default — sandboxed via `--allow-none`) or
    /// `node` (requires operator-supplied containment).
    #[arg(
        long,
        env = "XIAOGUAI_MCP_EXEC_JS__RUNTIME",
        default_value = "deno",
        value_parser = clap::value_parser!(Runtime),
    )]
    runtime: Runtime,

    /// Path to the runtime executable. Defaults to the runtime's name on
    /// `$PATH` (`deno` or `node`).
    #[arg(long, env = "XIAOGUAI_MCP_EXEC_JS__RUNTIME_BIN")]
    runtime_bin: Option<PathBuf>,

    /// Disable stderr PII redaction. Off by default — the agent-facing
    /// posture is to scrub.
    #[arg(long, env = "XIAOGUAI_MCP_EXEC_JS__NO_REDACT")]
    no_redact_stderr: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Log to stderr only — stdout is reserved for MCP framing.
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    let cli = Cli::parse();
    let runtime_bin = cli
        .runtime_bin
        .unwrap_or_else(|| PathBuf::from(cli.runtime.default_bin()));
    let cfg = ExecConfig {
        max_timeout: Duration::from_secs(cli.timeout_secs),
        memory_mb: cli.memory_mb,
        workdir_parent: cli.workdir_parent.unwrap_or_else(std::env::temp_dir),
        runtime: cli.runtime,
        runtime_bin,
        redact_stderr: !cli.no_redact_stderr,
    };
    tracing::info!(
        timeout_secs = cli.timeout_secs,
        memory_mb = cli.memory_mb,
        runtime = ?cli.runtime,
        runtime_bin = %cfg.runtime_bin.display(),
        redact_stderr = cfg.redact_stderr,
        "xiaoguai-mcp-exec-js: starting stdio server"
    );

    run_stdio_server(cfg).await
}
