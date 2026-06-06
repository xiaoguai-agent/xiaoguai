//! `ExecBackend` trait and the L1-JavaScript implementation
//! (`ProcessL1JavaScript`).
//!
//! This is the DEC-019 refactor seam, mirrored from
//! `xiaoguai-mcp-exec::runtime`. The trait is intentionally re-defined
//! in both L1 crates so the two trust boundaries stay independent —
//! see DEC-019 §"trade-offs" and the §6 plan adjustment in
//! `docs/plans/2026-05-30-sprint8-track-a-l3-sandbox.md`.

use std::time::Duration;

use async_trait::async_trait;

use crate::exec::{run_javascript, ExecConfig, ExecError, ExecResult};

/// Compact, agent-facing summary of what a backend can and cannot do.
///
/// See the Python crate's [`xiaoguai_mcp_exec::CapabilitySummary`] for
/// the canonical comment block — this type is structurally identical;
/// the duplication is intentional (different trust boundary).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilitySummary {
    pub tier: &'static str,
    pub language: &'static str,
    pub network: bool,
    pub filesystem: bool,
    pub subprocess: bool,
    pub max_memory_mb: u64,
    pub max_timeout_secs: u64,
}

/// The unified async interface every JS sandbox backend implements.
#[async_trait]
pub trait ExecBackend: Send + Sync {
    fn name(&self) -> &'static str;
    async fn run(&self, snippet: &str, timeout: Duration) -> Result<ExecResult, ExecError>;
    fn capability_summary(&self) -> CapabilitySummary;
}

/// L1 JavaScript backend: wraps the existing [`run_javascript`] free
/// function. Default runtime is Deno; Node is opt-in via
/// [`ExecConfig::runtime`].
#[derive(Clone, Debug)]
pub struct ProcessL1JavaScript {
    cfg: ExecConfig,
}

impl ProcessL1JavaScript {
    #[must_use]
    pub fn new(cfg: ExecConfig) -> Self {
        Self { cfg }
    }

    /// Borrow the underlying config — exposed for tests and the
    /// `server.rs` adapter.
    #[must_use]
    pub fn config(&self) -> &ExecConfig {
        &self.cfg
    }
}

#[async_trait]
impl ExecBackend for ProcessL1JavaScript {
    fn name(&self) -> &'static str {
        "process-l1-javascript"
    }

    async fn run(&self, snippet: &str, timeout: Duration) -> Result<ExecResult, ExecError> {
        run_javascript(&self.cfg, snippet, timeout).await
    }

    fn capability_summary(&self) -> CapabilitySummary {
        CapabilitySummary {
            tier: "L1",
            language: "javascript",
            // Honesty: Deno `--allow-none` denies network, but the Node runtime
            // has no permission model — `fetch`/`http`/`net` all work and there
            // is no netns/seccomp in this crate. Report `false` ONLY for Deno so
            // a policy/model keying on `network` isn't misled under Node.
            network: matches!(self.cfg.runtime, crate::exec::Runtime::Node),
            // Deno `--allow-none` blocks FS; Node has full ABI access
            // but operator is expected to mount FS read-only. We report
            // `true` because L1 in general can touch the tempdir CWD —
            // the deny-by-default posture only applies under Deno.
            filesystem: true,
            // Same reasoning as Python L1 — the interpreter is the
            // process, and Node has no `--allow-none` to deny child
            // spawn.
            subprocess: true,
            max_memory_mb: self.cfg.memory_mb,
            max_timeout_secs: self.cfg.max_timeout.as_secs(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::Runtime;
    use std::path::PathBuf;

    fn cfg_with(memory_mb: u64, timeout_secs: u64) -> ExecConfig {
        ExecConfig {
            max_timeout: Duration::from_secs(timeout_secs),
            memory_mb,
            workdir_parent: std::env::temp_dir(),
            runtime: Runtime::Deno,
            runtime_bin: PathBuf::from("deno"),
            redact_stderr: true,
        }
    }

    /// Test-only PATH probe; mirrors `exec::runtime_on_path` so we don't
    /// have to expose that helper publicly.
    fn runtime_on_path(bin: &str) -> bool {
        let Some(path) = std::env::var_os("PATH") else {
            return false;
        };
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(bin);
            if candidate.is_file() {
                return true;
            }
        }
        false
    }

    #[tokio::test]
    async fn process_l1_javascript_run_happy_path() {
        let backend = ProcessL1JavaScript::new(cfg_with(1024, 10));
        let bin = backend.config().runtime_bin.to_string_lossy().into_owned();
        if !runtime_on_path(&bin) {
            eprintln!("SKIPPED: {bin} not on PATH (install the runtime to exercise this test)");
            return;
        }
        let r = backend
            .run("console.log('ok')", Duration::from_secs(10))
            .await
            .expect("runtime spawned");
        assert_eq!(r.exit_code, Some(0), "stderr was: {}", r.stderr);
        assert_eq!(r.stdout.trim(), "ok");
        assert!(!r.timed_out);
    }

    #[test]
    fn process_l1_javascript_name_is_stable() {
        let backend = ProcessL1JavaScript::new(cfg_with(1024, 10));
        assert_eq!(backend.name(), "process-l1-javascript");
    }

    #[test]
    fn process_l1_javascript_capability_summary_is_l1() {
        let backend = ProcessL1JavaScript::new(cfg_with(1024, 10));
        let cap = backend.capability_summary();
        assert_eq!(cap.tier, "L1");
        assert_eq!(cap.language, "javascript");
        assert!(!cap.network);
        assert!(cap.filesystem);
        assert!(cap.subprocess);
    }

    #[tokio::test]
    async fn process_l1_javascript_run_rejects_oversize_snippet() {
        let backend = ProcessL1JavaScript::new(cfg_with(1024, 10));
        let huge = "x".repeat(64 * 1024 + 1);
        let err = backend
            .run(&huge, Duration::from_secs(5))
            .await
            .expect_err("oversized snippet must be rejected");
        assert!(matches!(err, ExecError::SnippetTooLarge(_)));
    }

    #[test]
    fn process_l1_javascript_capability_reflects_config_caps() {
        let backend = ProcessL1JavaScript::new(cfg_with(256, 7));
        let cap = backend.capability_summary();
        assert_eq!(cap.max_memory_mb, 256);
        assert_eq!(cap.max_timeout_secs, 7);
    }
}
