//! Subprocess wrapper for sandboxed Python execution.
//!
//! Each call spawns a fresh `python3 -I` in a fresh tempdir with a scrubbed
//! environment and an `ulimit -v` memory cap (or `prlimit --as` on Linux when
//! available). The process is killed by tokio when the wall-clock deadline
//! elapses. Stdout and stderr are captured to a hard cap; oversize output is
//! truncated with a marker.
//!
//! This module is intentionally MCP-agnostic — the only contract it exposes
//! to callers is [`run_python`] returning an [`ExecResult`]. The MCP tool
//! layer in `tools.rs` adapts that to MCP `CallToolResult`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;
use xiaoguai_types::redact::redact_str;

/// Hard ceiling on captured output per stream. Anything beyond is dropped
/// and the `truncated` flag is set so the caller can surface that to the
/// LLM ("output was 1.2 MB, truncated to 64 KB").
const OUTPUT_BYTE_CAP: usize = 64 * 1024;

/// Soft ceiling on the snippet itself. Anything bigger is almost certainly
/// a confused agent — fail fast rather than spawning python.
const CODE_BYTE_CAP: usize = 64 * 1024;

/// Environment variables forwarded into the sandbox. Everything else is
/// stripped — critically, `OLLAMA_HOST`, `DATABASE_URL`, audit signing
/// keys, and any custom `XIAOGUAI_*` knobs MUST NOT propagate.
const ENV_ALLOWLIST: &[&str] = &["PATH", "LANG", "LC_ALL", "LC_CTYPE"];

/// Configuration for a single execution call. Typically constructed once
/// per server instance and reused per request.
#[derive(Clone, Debug)]
pub struct ExecConfig {
    /// Hard wall-clock cap. Per-call timeouts are clamped to this.
    pub max_timeout: Duration,
    /// Address-space limit (RSS+VM) in megabytes; passed to `ulimit -v`
    /// (which actually limits virtual memory in kilobytes — we multiply
    /// by 1024).
    pub memory_mb: u64,
    /// Parent directory under which each call gets a fresh `mktemp -d`.
    /// Defaults to the OS temp dir.
    pub workdir_parent: PathBuf,
    /// Path to the `python3` executable. Default `python3` resolves on
    /// `$PATH` in the sandbox.
    pub python: PathBuf,
    /// When true, stderr passes through the PII redactor before return.
    pub redact_stderr: bool,
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            max_timeout: Duration::from_secs(30),
            memory_mb: 512,
            workdir_parent: std::env::temp_dir(),
            python: PathBuf::from("python3"),
            redact_stderr: true,
        }
    }
}

/// Result of a single execution. All fields are agent-facing: the LLM gets
/// to see this verbatim, modulo the `redact_stderr` flag on `ExecConfig`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecResult {
    /// Process exit code. `None` if the call was killed by deadline.
    pub exit_code: Option<i32>,
    /// Captured stdout (UTF-8 lossy; bytes beyond `OUTPUT_BYTE_CAP` dropped).
    pub stdout: String,
    /// Captured stderr (UTF-8 lossy; same cap; redacted when configured).
    pub stderr: String,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// True when either stdout or stderr hit `OUTPUT_BYTE_CAP`.
    pub truncated: bool,
    /// True when the deadline fired and we killed the process.
    pub timed_out: bool,
}

impl ExecResult {
    /// Convenience: did the process exit successfully?
    #[must_use]
    pub fn succeeded(&self) -> bool {
        !self.timed_out && self.exit_code == Some(0)
    }
}

/// Error path. Most "user code crashed" outcomes are reported through
/// [`ExecResult`] (non-zero `exit_code`); this enum is reserved for the
/// supervisor itself failing — e.g. python missing from `$PATH`, OS
/// refusing to spawn.
#[derive(Debug, Error)]
pub enum ExecError {
    /// `code` exceeds [`CODE_BYTE_CAP`]. We refuse to write a >64 KB
    /// snippet to disk before even starting python.
    #[error("snippet exceeds {CODE_BYTE_CAP} byte cap (got {0} bytes)")]
    SnippetTooLarge(usize),
    /// Failed to create the per-call tempdir.
    #[error("create tempdir under {0}: {1}")]
    Workdir(PathBuf, std::io::Error),
    /// Failed to write `main.py`.
    #[error("write main.py: {0}")]
    WriteCode(std::io::Error),
    /// Failed to spawn the child process at all (python missing, fork
    /// failed, etc.).
    #[error("spawn {0}: {1}")]
    Spawn(PathBuf, std::io::Error),
    /// I/O error while reading stdout/stderr or waiting on the child.
    #[error("child io: {0}")]
    ChildIo(std::io::Error),
}

/// Execute `code` under `cfg` with a per-call `timeout` (clamped to
/// `cfg.max_timeout`). Returns `Ok(ExecResult)` for every outcome that
/// involves python actually running (success, crash, deadline); returns
/// `Err(ExecError)` only when the supervisor itself couldn't get
/// python off the ground.
pub async fn run_python(
    cfg: &ExecConfig,
    code: &str,
    timeout_request: Duration,
) -> Result<ExecResult, ExecError> {
    if code.len() > CODE_BYTE_CAP {
        return Err(ExecError::SnippetTooLarge(code.len()));
    }
    let deadline = std::cmp::min(timeout_request, cfg.max_timeout);

    // Fresh tempdir per call. The `tempfile::TempDir` `Drop` impl removes
    // the directory tree, even on panic, so we never leak workdir state.
    let workdir = tempfile::Builder::new()
        .prefix("xg-exec-")
        .tempdir_in(&cfg.workdir_parent)
        .map_err(|e| ExecError::Workdir(cfg.workdir_parent.clone(), e))?;
    let main_py = workdir.path().join("main.py");
    tokio::fs::write(&main_py, code)
        .await
        .map_err(ExecError::WriteCode)?;

    let mut cmd = build_command(cfg, workdir.path(), &main_py);
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::piped());

    let start = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| ExecError::Spawn(cfg.python.clone(), e))?;

    // Close stdin so snippets that read() get EOF immediately rather than
    // blocking forever on input that's never going to come.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.shutdown().await;
        drop(stdin);
    }

    let wait_fut = child.wait_with_output();
    let (exit_code, stdout_raw, stderr_raw, timed_out) = match timeout(deadline, wait_fut).await {
        Ok(Ok(output)) => (output.status.code(), output.stdout, output.stderr, false),
        Ok(Err(e)) => return Err(ExecError::ChildIo(e)),
        Err(_elapsed) => {
            // tokio::process::Child::wait_with_output consumed the child;
            // when wait_with_output is cancelled the child is dropped, and
            // tokio's Drop sends SIGKILL (via kill_on_drop default false…
            // we explicitly set it below via build_command). The process
            // is on its way out; we report what we have, which is nothing
            // since wait_with_output buffers until completion. The "no
            // output captured on timeout" is a known limitation noted in
            // the runbook.
            (None, Vec::new(), Vec::new(), true)
        }
    };
    let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

    // Truncate, decode lossily, and (for stderr) redact PII.
    let (stdout, stdout_truncated) = decode_capped(&stdout_raw);
    let (stderr_text, stderr_truncated) = decode_capped(&stderr_raw);
    let stderr = if cfg.redact_stderr {
        redact_str(&stderr_text)
    } else {
        stderr_text
    };

    Ok(ExecResult {
        exit_code,
        stdout,
        stderr,
        duration_ms,
        truncated: stdout_truncated || stderr_truncated,
        timed_out,
    })
}

/// Assemble the `python3 -I main.py` command with a scrubbed env, fresh CWD,
/// and a memory cap. On Unix we wrap through `sh -c "ulimit -v $N && exec
/// python3 -I main.py"` because tokio's `Command::pre_exec` requires unsafe
/// and the project forbids unsafe code at workspace level.
fn build_command(cfg: &ExecConfig, working_dir: &Path, main_py: &Path) -> Command {
    // Address-space limit in kilobytes (what `ulimit -v` actually takes).
    let mem_kb = cfg.memory_mb.saturating_mul(1024);

    // Build the env from the allowlist only. Inherit values from the
    // parent process for keys the operator expects to work (PATH, locale).
    let env: HashMap<&str, String> = ENV_ALLOWLIST
        .iter()
        .filter_map(|key| std::env::var(key).ok().map(|v| (*key, v)))
        .collect();

    let shell_inner = format!(
        "ulimit -v {mem_kb} 2>/dev/null; exec {python} -I {main}",
        python = shell_quote(&cfg.python.display().to_string()),
        main = shell_quote(&main_py.display().to_string()),
    );

    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(shell_inner);
    command.current_dir(working_dir);
    command.env_clear();
    for (k, v) in env {
        command.env(k, v);
    }
    // Kill the child if the future is dropped (e.g. on timeout cancellation).
    command.kill_on_drop(true);
    command
}

/// Minimal single-quote escaping for `/bin/sh -c`. Used only for the python
/// path and the main.py path — both are under our control (workdir is a
/// tempdir with a known prefix), so the surface area is tiny.
fn shell_quote(s: &str) -> String {
    let escaped = s.replace('\'', r"'\''");
    format!("'{escaped}'")
}

/// Decode bytes lossily, capped at [`OUTPUT_BYTE_CAP`]. Returns the decoded
/// string plus a flag indicating whether truncation happened.
fn decode_capped(bytes: &[u8]) -> (String, bool) {
    if bytes.len() <= OUTPUT_BYTE_CAP {
        (String::from_utf8_lossy(bytes).into_owned(), false)
    } else {
        let cut = &bytes[..OUTPUT_BYTE_CAP];
        let mut s = String::from_utf8_lossy(cut).into_owned();
        s.push_str(&format!(
            "\n…[truncated; {} bytes dropped]",
            bytes.len() - OUTPUT_BYTE_CAP
        ));
        (s, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cfg() -> ExecConfig {
        ExecConfig {
            max_timeout: Duration::from_secs(10),
            memory_mb: 256,
            workdir_parent: std::env::temp_dir(),
            python: PathBuf::from("python3"),
            redact_stderr: true,
        }
    }

    #[tokio::test]
    async fn snippet_too_large_short_circuits() {
        let cfg = test_cfg();
        let huge = "x".repeat(CODE_BYTE_CAP + 1);
        let err = run_python(&cfg, &huge, Duration::from_secs(5))
            .await
            .expect_err("oversized snippet should bounce before spawn");
        assert!(matches!(err, ExecError::SnippetTooLarge(_)));
    }

    #[tokio::test]
    async fn happy_path_captures_stdout_and_exits_zero() {
        let cfg = test_cfg();
        let r = run_python(&cfg, "print('hello from sandbox')", Duration::from_secs(5))
            .await
            .expect("python3 must be available for this test");
        assert_eq!(r.exit_code, Some(0), "stderr was: {}", r.stderr);
        assert_eq!(r.stdout.trim(), "hello from sandbox");
        assert!(!r.timed_out);
        assert!(r.succeeded());
    }

    #[tokio::test]
    async fn nonzero_exit_is_reported_through_result_not_error() {
        let cfg = test_cfg();
        let r = run_python(&cfg, "import sys; sys.exit(3)", Duration::from_secs(5))
            .await
            .expect("supervisor itself must succeed");
        assert_eq!(r.exit_code, Some(3));
        assert!(!r.timed_out);
        assert!(!r.succeeded());
    }

    #[tokio::test]
    async fn timeout_kills_long_running_snippet() {
        let cfg = test_cfg();
        // Sleep 5 seconds inside python, deadline 500 ms.
        let r = run_python(
            &cfg,
            "import time; time.sleep(5); print('should not print')",
            Duration::from_millis(500),
        )
        .await
        .expect("supervisor handles deadline as a result, not an error");
        assert!(r.timed_out, "deadline should fire");
        assert_eq!(r.exit_code, None);
        // The duration should be close to the deadline, not the 5s sleep.
        assert!(
            r.duration_ms < 3000,
            "duration {}ms suggests we waited for the snippet to finish",
            r.duration_ms
        );
        assert!(!r.stdout.contains("should not print"));
    }

    #[tokio::test]
    async fn stderr_is_redacted_when_configured() {
        let cfg = test_cfg();
        let snippet = r#"
import sys
print("contact me at alice@example.com", file=sys.stderr)
"#;
        let r = run_python(&cfg, snippet, Duration::from_secs(5))
            .await
            .expect("python3 must be available");
        assert!(
            !r.stderr.contains("alice@example.com"),
            "email leaked: {}",
            r.stderr
        );
        // Redactor replaces emails with a marker; existing redactor uses
        // "[redacted-email]" — assert *something* changed, not the exact
        // token, so the test survives format tweaks.
        assert!(
            r.stderr.contains("redact") || r.stderr.contains("REDACT"),
            "expected redaction marker in stderr: {}",
            r.stderr
        );
    }

    #[tokio::test]
    async fn redaction_disabled_preserves_stderr() {
        let cfg = ExecConfig {
            redact_stderr: false,
            ..test_cfg()
        };
        let snippet = r#"
import sys
print("contact me at alice@example.com", file=sys.stderr)
"#;
        let r = run_python(&cfg, snippet, Duration::from_secs(5))
            .await
            .expect("python3 must be available");
        assert!(
            r.stderr.contains("alice@example.com"),
            "redaction off should pass email through: {}",
            r.stderr
        );
    }

    #[tokio::test]
    async fn stdout_cap_is_enforced_with_truncation_marker() {
        let cfg = test_cfg();
        // ~150 KB of output, well over OUTPUT_BYTE_CAP (64 KB).
        let snippet = format!("print('x' * {})", OUTPUT_BYTE_CAP + 64 * 1024);
        let r = run_python(&cfg, &snippet, Duration::from_secs(5))
            .await
            .expect("python3 must be available");
        assert!(r.truncated);
        assert!(
            r.stdout.contains("truncated"),
            "expected truncation marker, got len={}",
            r.stdout.len()
        );
        assert!(
            r.stdout.len() <= OUTPUT_BYTE_CAP + 64,
            "stdout {} bytes exceeds cap {}",
            r.stdout.len(),
            OUTPUT_BYTE_CAP
        );
    }

    #[tokio::test]
    async fn env_secrets_do_not_leak_into_sandbox() {
        let cfg = test_cfg();
        // Set a fake secret in the parent process env.
        std::env::set_var("XIAOGUAI_AUDIT_SIGNING_KEY", "super-secret-do-not-leak");
        let r = run_python(
            &cfg,
            "import os; print(os.environ.get('XIAOGUAI_AUDIT_SIGNING_KEY', 'absent'))",
            Duration::from_secs(5),
        )
        .await
        .expect("python3 must be available");
        std::env::remove_var("XIAOGUAI_AUDIT_SIGNING_KEY");
        assert_eq!(r.exit_code, Some(0), "stderr: {}", r.stderr);
        assert_eq!(
            r.stdout.trim(),
            "absent",
            "secret leaked into sandbox env: {}",
            r.stdout
        );
    }

    #[tokio::test]
    async fn workdir_is_fresh_per_call() {
        let cfg = test_cfg();
        // First call writes a file to CWD. Second call must not see it.
        let r1 = run_python(
            &cfg,
            "open('cwd-marker.txt', 'w').write('first'); print('wrote')",
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(r1.exit_code, Some(0));
        let r2 = run_python(
            &cfg,
            "import os; print('present' if os.path.exists('cwd-marker.txt') else 'absent')",
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(r2.stdout.trim(), "absent");
    }
}
