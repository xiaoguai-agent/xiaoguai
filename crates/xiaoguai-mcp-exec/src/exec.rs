//! Subprocess wrapper for sandboxed Python execution.
//!
//! Each call spawns a fresh `python3 -I` in a fresh tempdir with a scrubbed
//! environment and in-process rlimits (SEC-10, reworked in #289): a
//! `pre_exec` hook calls `setrlimit(2)` in the forked child — a best-effort
//! `RLIMIT_AS` memory cap (not settable on every platform, notably macOS)
//! plus **enforced** `RLIMIT_NPROC` process-count and `RLIMIT_FSIZE`
//! file-size caps — if those two cannot be applied, python is never exec'd.
//! Previously the caps rode a `/bin/sh -c "ulimit …"` preamble, but `ulimit`
//! semantics diverge under dash (Ubuntu's `/bin/sh`), so the limits could
//! silently not apply; `setrlimit` removes the shell dependency entirely.
//! The process is killed by tokio when the wall-clock deadline elapses.
//! Stdout and stderr are captured to a hard cap; oversize output is
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

/// SEC-10: cap on the number of processes the snippet's user may run
/// (`RLIMIT_NPROC`) — the fork-bomb guard. Enforced: a failure to apply it
/// aborts the run (#289: the `pre_exec` hook returns `Err`, so `spawn`
/// fails and python never execs). Note `RLIMIT_NPROC` counts ALL processes
/// of the daemon's UID, not just the sandbox's children, so the value must
/// comfortably exceed a desktop user's baseline (often 400–600 on macOS) or
/// legitimate single `subprocess` use inside snippets starts failing — a
/// fork bomb tries to spawn thousands, so 1024 stops it just as well.
const MAX_SUBPROCS: u64 = 1024;

/// SEC-10: cap on the size of any single file the snippet may write
/// (`RLIMIT_FSIZE`) — the disk-fill guard. Enforced: a failure to apply it
/// aborts the run, same as [`MAX_SUBPROCS`]. In **bytes** (2 GiB) — #289:
/// `setrlimit` takes bytes directly, unlike the retired `ulimit -f`
/// preamble which counted 512-byte blocks.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Configuration for a single execution call. Typically constructed once
/// per server instance and reused per request.
#[derive(Clone, Debug)]
pub struct ExecConfig {
    /// Hard wall-clock cap. Per-call timeouts are clamped to this.
    pub max_timeout: Duration,
    /// Address-space limit (virtual memory) in megabytes; applied as
    /// `RLIMIT_AS` via `setrlimit` in the child (#289). Best-effort: not
    /// every platform lets us set it (notably macOS), and a failure to
    /// apply it is ignored rather than aborting the run.
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
    /// failed, etc.). #289: an enforced rlimit (`RLIMIT_NPROC`/`RLIMIT_FSIZE`)
    /// that cannot be applied also surfaces here — the `pre_exec` hook's
    /// `Err` aborts the child before exec, and std reports it as a spawn
    /// failure. No silent fallback to an uncapped run.
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

    // Capture the pid before `wait_with_output` consumes the child — on timeout
    // we use it (as the process-group id, since the child is its own group
    // leader) to reap forked grandchildren, not just the top pid.
    let child_pid = child.id();
    let wait_fut = child.wait_with_output();
    let (exit_code, stdout_raw, stderr_raw, timed_out) = match timeout(deadline, wait_fut).await {
        Ok(Ok(output)) => (output.status.code(), output.stdout, output.stderr, false),
        Ok(Err(e)) => return Err(ExecError::ChildIo(e)),
        Err(_elapsed) => {
            // Dropping the cancelled `wait_with_output` future SIGKILLs the top
            // pid (kill_on_drop), but forked grandchildren survive. The child is
            // its own group leader (process_group(0)), so SIGKILL the whole
            // group `-pgid` to reap them. Best-effort via /bin/kill — #289
            // scoped unsafe to the two setrlimit sites only, so a direct
            // `libc::kill` stays out; the external binary is fine here.
            // No output is captured on timeout (wait_with_output buffers until
            // completion) — a known limitation noted in the runbook.
            #[cfg(unix)]
            if let Some(pid) = child_pid {
                let _ = tokio::process::Command::new("kill")
                    .arg("-KILL")
                    .arg(format!("-{pid}"))
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
            }
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
/// and resource caps.
///
/// #289: python is invoked DIRECTLY — the former `sh -c "ulimit …; exec
/// python3 -I main.py"` wrapper is gone. `ulimit` is a shell builtin whose
/// `-u`/`-f` semantics diverge under dash (Ubuntu's `/bin/sh`), which made
/// the SEC-10 caps unreliable; the shell carried no other responsibility
/// (quoting, env, and CWD were always handled on the Rust side). The caps
/// are now applied in-process via a `pre_exec` hook calling `setrlimit(2)`
/// in the forked child, before exec:
///  * `RLIMIT_AS` ([`ExecConfig::memory_mb`]) is BEST-EFFORT — several
///    platforms (notably macOS) refuse it, and failing the whole run there
///    would break every host. Failure is ignored.
///  * `RLIMIT_NPROC` ([`MAX_SUBPROCS`], fork-bomb guard) and `RLIMIT_FSIZE`
///    ([`MAX_FILE_BYTES`], disk-fill guard) are ENFORCED: if either cannot
///    be set, the hook returns `Err`, the child aborts before exec, and the
///    caller sees [`ExecError::Spawn`] — no silent fallback to an uncapped
///    run.
fn build_command(cfg: &ExecConfig, working_dir: &Path, main_py: &Path) -> Command {
    // Build the env from the allowlist only. Inherit values from the
    // parent process for keys the operator expects to work (PATH, locale).
    let env: HashMap<&str, String> = ENV_ALLOWLIST
        .iter()
        .filter_map(|key| std::env::var(key).ok().map(|v| (*key, v)))
        .collect();

    let mut command = Command::new(&cfg.python);
    command.arg("-I").arg(main_py);
    command.current_dir(working_dir);
    command.env_clear();
    for (k, v) in env {
        command.env(k, v);
    }

    // #289 (SEC-10): apply the rlimits between fork and exec.
    #[cfg(unix)]
    {
        let mem_bytes = cfg.memory_mb.saturating_mul(1024 * 1024);
        // SAFETY: `pre_exec` runs in the forked child before exec, where
        // only async-signal-safe operations are permitted. The closure
        // calls nothing but `setrlimit(2)` (async-signal-safe per POSIX)
        // and constructs `io::Error` values from raw OS errors — no
        // allocation, no locks, no other process state touched.
        #[allow(unsafe_code)]
        unsafe {
            command.pre_exec(move || {
                // Best-effort address-space cap; see build_command docs.
                let _ = set_rlimit(libc::RLIMIT_AS, mem_bytes);
                // Enforced caps: bubble the error so spawn fails loudly.
                set_rlimit(libc::RLIMIT_NPROC, MAX_SUBPROCS)?;
                set_rlimit(libc::RLIMIT_FSIZE, MAX_FILE_BYTES)?;
                Ok(())
            });
        }
    }

    // Kill the child if the future is dropped (e.g. on timeout cancellation).
    command.kill_on_drop(true);
    // Put the child in its own process group (it becomes group leader, so
    // pgid == child pid) so a timeout can SIGKILL the WHOLE group — otherwise
    // kill_on_drop only reaps the top python pid and any forked
    // grandchildren (subprocess/multiprocessing) survive as orphans.
    #[cfg(unix)]
    command.process_group(0);
    command
}

/// Numeric type of the `RLIMIT_*` constants: glibc declares them as
/// `__rlimit_resource_t` (a `c_uint`) while macOS, musl, and the BSDs use
/// `c_int` (#289).
#[cfg(all(target_os = "linux", target_env = "gnu"))]
type RlimitResource = libc::__rlimit_resource_t;
#[cfg(all(unix, not(all(target_os = "linux", target_env = "gnu"))))]
type RlimitResource = libc::c_int;

/// Set both the soft and hard bound of `resource` to `limit` (#289,
/// SEC-10). Hard too, deliberately: the snippet runs as the same UID and
/// could otherwise raise its soft limit right back up via
/// `resource.setrlimit`.
///
/// Called from the `pre_exec` hook between fork and exec — keep this
/// async-signal-safe (no allocation, no locks).
#[cfg(unix)]
fn set_rlimit(resource: RlimitResource, limit: u64) -> std::io::Result<()> {
    let rlim = libc::rlimit {
        rlim_cur: limit as libc::rlim_t,
        rlim_max: limit as libc::rlim_t,
    };
    // SAFETY: `rlim` is a valid, fully-initialized struct that outlives the
    // call; `setrlimit` only reads through the pointer.
    #[allow(unsafe_code)]
    let rc = unsafe { libc::setrlimit(resource, std::ptr::addr_of!(rlim)) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
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

    /// SEC-10: the enforced `RLIMIT_NPROC` cap is a fork-bomb guard, not a
    /// subprocess ban — a single legitimate child must keep working (the
    /// capability summary advertises `subprocess: true`). This also proves
    /// the `pre_exec` rlimit hook applied cleanly (#289: a failed
    /// `RLIMIT_NPROC`/`RLIMIT_FSIZE` set aborts before exec-ing python).
    ///
    /// A true fork-bomb test (spawn until `EAGAIN`) is deliberately omitted:
    /// it would be slow and flaky on shared runners because `RLIMIT_NPROC`
    /// counts every process of the runner's UID.
    #[tokio::test]
    async fn subprocess_single_child_still_works_under_nproc_cap() {
        let cfg = test_cfg();
        let snippet = r#"
import subprocess, sys
r = subprocess.run([sys.executable, "-I", "-c", "print('child-ok')"], capture_output=True, text=True)
print(r.stdout.strip())
"#;
        let r = run_python(&cfg, snippet, Duration::from_secs(10))
            .await
            .expect("python3 must be available");
        assert_eq!(r.exit_code, Some(0), "stderr: {}", r.stderr);
        assert_eq!(r.stdout.trim(), "child-ok");
    }

    /// #289: the in-process `setrlimit` hook must actually take effect
    /// inside the child — observe the limits from python's `resource`
    /// module rather than triggering them (a real fork bomb or a 2 GiB
    /// write would be slow and flaky on shared runners). Both soft and
    /// hard bounds are asserted: the hard bound is what stops a snippet
    /// from raising its own soft limit back up.
    #[cfg(unix)]
    #[tokio::test]
    async fn rlimits_are_applied_inside_the_child() {
        let cfg = test_cfg();
        let snippet = r"
import resource
for lim in (resource.RLIMIT_NPROC, resource.RLIMIT_FSIZE):
    soft, hard = resource.getrlimit(lim)
    print(soft)
    print(hard)
";
        let r = run_python(&cfg, snippet, Duration::from_secs(5))
            .await
            .expect("python3 must be available");
        assert_eq!(r.exit_code, Some(0), "stderr: {}", r.stderr);
        let got: Vec<&str> = r.stdout.split_whitespace().collect();
        let nproc = MAX_SUBPROCS.to_string();
        let fsize = MAX_FILE_BYTES.to_string();
        assert_eq!(
            got,
            vec![
                nproc.as_str(),
                nproc.as_str(),
                fsize.as_str(),
                fsize.as_str()
            ],
            "child rlimits do not match the SEC-10 caps; stdout: {}",
            r.stdout
        );
    }

    /// #289: `RLIMIT_FSIZE` must hold even though `RLIMIT_AS` is
    /// best-effort — the snippet lowers its own soft FSIZE bound (always
    /// permitted, no privileges needed) and proves an over-limit write
    /// fails. This exercises the same enforcement path the 2 GiB hard cap
    /// relies on, without writing 2 GiB on a shared runner. python turns
    /// the kernel's `SIGXFSZ`/`EFBIG` into `OSError`, which the snippet expects.
    #[cfg(unix)]
    #[tokio::test]
    async fn fsize_limit_blocks_oversize_writes() {
        let cfg = test_cfg();
        let snippet = r#"
import resource, signal
# Don't die on SIGXFSZ; make the over-limit write fail with EFBIG instead.
signal.signal(signal.SIGXFSZ, signal.SIG_IGN)
resource.setrlimit(resource.RLIMIT_FSIZE, (4096, resource.getrlimit(resource.RLIMIT_FSIZE)[1]))
try:
    with open("big.bin", "wb") as f:
        f.write(b"x" * 8192)
        f.flush()
    print("write-succeeded")
except OSError:
    print("write-blocked")
"#;
        let r = run_python(&cfg, snippet, Duration::from_secs(5))
            .await
            .expect("python3 must be available");
        assert_eq!(r.exit_code, Some(0), "stderr: {}", r.stderr);
        assert_eq!(
            r.stdout.trim(),
            "write-blocked",
            "RLIMIT_FSIZE did not stop an over-limit write"
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
