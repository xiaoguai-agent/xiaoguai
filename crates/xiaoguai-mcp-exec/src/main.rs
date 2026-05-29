//! `xiaoguai-mcp-exec` binary — stdio MCP transport for the sandbox.
//!
//! Reads MCP protocol from stdin, writes results to stdout. Intended to be
//! spawned by an MCP supervisor (xiaoguai's [`McpSupervisor`] or any other
//! compliant client) with `transport=stdio`.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use xiaoguai_mcp_exec::{run_stdio_server, ExecConfig};

#[derive(Parser, Debug)]
#[command(
    name = "xiaoguai-mcp-exec",
    version,
    about = "Sandboxed code-execution MCP server"
)]
struct Cli {
    /// Hard wall-clock cap per call (seconds). Per-call timeouts above this
    /// are clamped.
    #[arg(long, env = "XIAOGUAI_MCP_EXEC__TIMEOUT_SECS", default_value_t = 30)]
    timeout_secs: u64,

    /// Address-space limit (megabytes) per call. Passed to `ulimit -v`.
    #[arg(long, env = "XIAOGUAI_MCP_EXEC__MEMORY_MB", default_value_t = 512)]
    memory_mb: u64,

    /// Parent directory for per-call tempdirs. Defaults to the OS temp dir.
    #[arg(long, env = "XIAOGUAI_MCP_EXEC__WORKDIR_PARENT")]
    workdir_parent: Option<PathBuf>,

    /// Path to `python3`. Default looks up on `$PATH` inside the sandbox.
    #[arg(long, env = "XIAOGUAI_MCP_EXEC__PYTHON", default_value = "python3")]
    python: PathBuf,

    /// Disable stderr PII redaction. Off by default — the agent-facing
    /// posture is to scrub.
    #[arg(long, env = "XIAOGUAI_MCP_EXEC__NO_REDACT")]
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
    let cfg = ExecConfig {
        max_timeout: Duration::from_secs(cli.timeout_secs),
        memory_mb: cli.memory_mb,
        workdir_parent: cli.workdir_parent.unwrap_or_else(std::env::temp_dir),
        python: cli.python,
        redact_stderr: !cli.no_redact_stderr,
    };
    tracing::info!(
        timeout_secs = cli.timeout_secs,
        memory_mb = cli.memory_mb,
        python = %cfg.python.display(),
        redact_stderr = cfg.redact_stderr,
        "xiaoguai-mcp-exec: starting stdio server"
    );

    run_stdio_server(cfg).await
}
