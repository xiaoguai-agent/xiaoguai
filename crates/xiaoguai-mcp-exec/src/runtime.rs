//! `ExecBackend` trait and the L1-Python implementation (`ProcessL1Python`).
//!
//! This is the DEC-019 refactor seam: future tiers (L3 wasmtime, L4
//! Firecracker) drop in by implementing the same trait. The trait is
//! intentionally re-defined in both L1 crates (Python and JavaScript) so
//! the two trust boundaries stay independent — see DEC-019 §"trade-offs"
//! and the §6 plan adjustment in
//! `docs/plans/2026-05-30-sprint8-track-a-l3-sandbox.md`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;

use crate::exec::{run_python, ExecConfig, ExecError, ExecResult};

/// SEC-09: env var an operator sets (any value other than empty/`0`/`false`)
/// to acknowledge that the L1 backend is **not** filesystem/network-isolated
/// and silence the one-time startup warning.
pub const ACK_UNISOLATED_ENV: &str = "XIAOGUAI_MCP_EXEC_ACK_UNISOLATED";

/// SEC-09: makes the un-isolation warning fire at most once per process —
/// L1 backends can be constructed per call site (server, tests, adapters)
/// and the warning would otherwise flood the log.
static UNISOLATED_WARNED: AtomicBool = AtomicBool::new(false);

/// SEC-09: emit a single, prominent warning that L1 provides *process-level*
/// containment only (scrubbed env, ulimits, tempdir CWD) — **no** filesystem
/// or network isolation. Behaviour is unchanged (L1 still runs); this is
/// operator awareness, not a gate. Silenced via [`ACK_UNISOLATED_ENV`].
fn warn_unisolated_once() {
    let acked = std::env::var(ACK_UNISOLATED_ENV)
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            !v.is_empty() && v != "0" && v != "false"
        })
        .unwrap_or(false);
    if acked || UNISOLATED_WARNED.swap(true, Ordering::Relaxed) {
        return;
    }
    tracing::warn!(
        backend = "process-l1-python",
        "SEC-09: the L1 exec backend does NOT isolate the filesystem or network — \
         LLM-generated code can read any file the daemon user can and open outbound \
         connections (exfiltration risk). For production, prefer the L3 wasm backend \
         or deploy under container/netns isolation. Set {ACK_UNISOLATED_ENV}=1 to \
         acknowledge the risk and silence this warning."
    );
}

/// Compact, agent-facing summary of what a backend can and cannot do.
///
/// Surfaced as part of the tool's `list_tools` response so the LLM can
/// route around capability gaps (e.g. "L3 has no subprocess; if you need
/// `os.system`, route to the L1 tool name instead").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilitySummary {
    /// Sandbox tier label: `"L1"` (process isolation) or `"L3"`
    /// (wasmtime capability sandbox).
    pub tier: &'static str,
    /// Language label: `"python"` or `"javascript"`.
    pub language: &'static str,
    /// True when the sandbox can reach the network from inside.
    /// Always `false` for both L1 and L3 in this crate.
    pub network: bool,
    /// True when the sandbox can read/write files. L1 has a scoped
    /// tempdir CWD; L3 has nothing at all.
    pub filesystem: bool,
    /// True when the sandbox can spawn further processes. L1 is the
    /// process itself (so `True` if you consider the interpreter as
    /// "spawn-able"); L3 cannot syscall at all so it is `False`.
    pub subprocess: bool,
    /// Memory ceiling visible to the snippet, in megabytes.
    pub max_memory_mb: u64,
    /// Wall-clock ceiling visible to the snippet, in whole seconds.
    pub max_timeout_secs: u64,
}

/// The unified async interface every sandbox backend implements.
///
/// `run` is intentionally async so wasmtime's `async_support` path is
/// reachable. L1 implementations bridge to their existing
/// `run_<lang>` free functions for backward compatibility.
#[async_trait]
pub trait ExecBackend: Send + Sync {
    /// Stable name, used in metrics labels and logs (e.g.
    /// `"process-l1-python"`, `"wasmtime-l3-python"`). Must NOT change
    /// between versions — operators key dashboards off it.
    fn name(&self) -> &'static str;

    /// Run `snippet` with the per-call wall-clock `timeout`. The backend
    /// is free to clamp the timeout against its own internal cap.
    async fn run(&self, snippet: &str, timeout: Duration) -> Result<ExecResult, ExecError>;

    /// Static summary of what this backend exposes to snippets. Used by
    /// the MCP tool description so agents can route by capability.
    fn capability_summary(&self) -> CapabilitySummary;
}

/// L1 Python backend: wraps the existing [`run_python`] free function.
#[derive(Clone, Debug)]
pub struct ProcessL1Python {
    cfg: ExecConfig,
}

impl ProcessL1Python {
    /// Construct from an [`ExecConfig`]. The config is cheap to clone
    /// (no `Arc` — see crate-level docs).
    #[must_use]
    pub fn new(cfg: ExecConfig) -> Self {
        // SEC-09: surface the isolation gap once per process at the point an
        // un-isolated (L1) backend is chosen.
        warn_unisolated_once();
        Self { cfg }
    }

    /// Borrow the underlying config; useful for tests and the
    /// `server.rs` adapter that constructs an `ExecServer` from the
    /// same source-of-truth.
    #[must_use]
    pub fn config(&self) -> &ExecConfig {
        &self.cfg
    }
}

#[async_trait]
impl ExecBackend for ProcessL1Python {
    fn name(&self) -> &'static str {
        "process-l1-python"
    }

    async fn run(&self, snippet: &str, timeout: Duration) -> Result<ExecResult, ExecError> {
        run_python(&self.cfg, snippet, timeout).await
    }

    fn capability_summary(&self) -> CapabilitySummary {
        CapabilitySummary {
            tier: "L1",
            // L1 is env-scrubbed + ulimit'd + tempdir-CWD, but does NOT isolate
            // the network (no netns/seccomp) — be honest so the model / any
            // policy keying on this isn't misled. Deploy under container/netns
            // egress isolation if outbound must be denied.
            network: true,
            language: "python",
            // L1 sees a scoped tempdir CWD.
            filesystem: true,
            // The python interpreter itself is the process; `os.system`
            // would fork another child. Reported as `true` so agents
            // know L1 can shell out (subject to ulimit + tempdir scope).
            subprocess: true,
            max_memory_mb: self.cfg.memory_mb,
            max_timeout_secs: self.cfg.max_timeout.as_secs(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn cfg_with(memory_mb: u64, timeout_secs: u64) -> ExecConfig {
        ExecConfig {
            max_timeout: Duration::from_secs(timeout_secs),
            memory_mb,
            workdir_parent: std::env::temp_dir(),
            python: PathBuf::from("python3"),
            redact_stderr: true,
        }
    }

    #[tokio::test]
    async fn process_l1_python_run_happy_path() {
        let backend = ProcessL1Python::new(cfg_with(256, 10));
        let r = backend
            .run("print('ok')", Duration::from_secs(5))
            .await
            .expect("python3 must be available for this test");
        assert_eq!(r.exit_code, Some(0), "stderr was: {}", r.stderr);
        assert_eq!(r.stdout.trim(), "ok");
        assert!(!r.timed_out);
    }

    #[test]
    fn process_l1_python_name_is_stable() {
        let backend = ProcessL1Python::new(cfg_with(256, 10));
        assert_eq!(backend.name(), "process-l1-python");
    }

    #[test]
    fn process_l1_python_capability_summary_is_l1() {
        let backend = ProcessL1Python::new(cfg_with(256, 10));
        let cap = backend.capability_summary();
        assert_eq!(cap.tier, "L1");
        assert_eq!(cap.language, "python");
        assert!(
            cap.network,
            "L1 does not isolate the network — must report true"
        );
        assert!(cap.filesystem);
        assert!(cap.subprocess);
    }

    #[tokio::test]
    async fn process_l1_python_run_rejects_oversize_snippet() {
        let backend = ProcessL1Python::new(cfg_with(256, 10));
        // CODE_BYTE_CAP is 64 KB; +1 trips the guard.
        let huge = "x".repeat(64 * 1024 + 1);
        let err = backend
            .run(&huge, Duration::from_secs(5))
            .await
            .expect_err("oversized snippet must be rejected");
        assert!(matches!(err, ExecError::SnippetTooLarge(_)));
    }

    #[test]
    fn process_l1_python_capability_reflects_config_caps() {
        let backend = ProcessL1Python::new(cfg_with(128, 5));
        let cap = backend.capability_summary();
        assert_eq!(cap.max_memory_mb, 128);
        assert_eq!(cap.max_timeout_secs, 5);
    }
}
