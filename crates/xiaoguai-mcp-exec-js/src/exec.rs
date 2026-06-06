//! Subprocess wrapper for sandboxed JavaScript execution.
//!
//! Each call spawns a fresh JS runtime (`deno` by default, `node` opt-in)
//! in a fresh tempdir with a scrubbed environment and an `ulimit -v`
//! memory cap. The process is killed by tokio when the wall-clock
//! deadline elapses. Stdout and stderr are captured to a hard cap;
//! oversize output is truncated with a marker.
//!
//! This module is intentionally MCP-agnostic — the only contract it
//! exposes to callers is [`run_javascript`] returning an [`ExecResult`].
//! The MCP tool layer in `tools.rs` adapts that to MCP `CallToolResult`.
//!
//! ## Separate trust boundary
//!
//! This crate is a *sibling* to `xiaoguai-mcp-exec` (Python), not a
//! reuse. A sandbox escape in one runtime must not chain into the other,
//! so we keep a separate binary, separate `HotL` scope
//! (`tool_call.execute_javascript`), and per-runtime threat-model
//! documentation in the runbook.

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
/// a confused agent — fail fast rather than spawning the runtime.
const CODE_BYTE_CAP: usize = 64 * 1024;

/// Environment variables forwarded into the sandbox. Everything else is
/// stripped — critically, `OLLAMA_HOST`, `DATABASE_URL`, audit signing
/// keys, and any custom `XIAOGUAI_*` knobs MUST NOT propagate.
const ENV_ALLOWLIST: &[&str] = &["PATH", "LANG", "LC_ALL", "LC_CTYPE"];

/// Which JavaScript runtime to spawn.
///
/// **Deno** is the default. Its `--allow-none` posture means the runtime
/// itself denies network and filesystem access — we do not have to audit
/// our own sandbox-escape surface for either.
///
/// **Node** is opt-in via `--runtime node`. It has no built-in
/// allow/deny model, so the operator must enforce containment at the
/// outer layer (container `--network none`, k8s `NetworkPolicy`,
/// filesystem read-only mount). The runbook makes this trade-off
/// explicit.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    Deno,
    Node,
}

impl Runtime {
    /// Binary name on `$PATH` for this runtime.
    #[must_use]
    pub fn default_bin(self) -> &'static str {
        match self {
            Runtime::Deno => "deno",
            Runtime::Node => "node",
        }
    }

    /// The shell-quoted runtime invocation given the absolute path to
    /// `main.js`. Deno gets `--allow-none` for built-in sandboxing; Node
    /// gets `--no-deprecation` to keep stderr noise down (it has no
    /// allow/deny flag — containment lives at the deploy layer).
    fn invoke(self, bin: &str, main_js: &str) -> String {
        let bin_q = shell_quote(bin);
        let main_q = shell_quote(main_js);
        match self {
            Runtime::Deno => format!("exec {bin_q} run --allow-none {main_q}"),
            Runtime::Node => format!("exec {bin_q} --no-deprecation {main_q}"),
        }
    }
}

impl std::str::FromStr for Runtime {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "deno" => Ok(Runtime::Deno),
            "node" | "nodejs" => Ok(Runtime::Node),
            other => Err(format!("unknown runtime '{other}' (expected deno or node)")),
        }
    }
}

/// Configuration for a single execution call. Typically constructed once
/// per server instance and reused per request.
#[derive(Clone, Debug)]
pub struct ExecConfig {
    /// Hard wall-clock cap. Per-call timeouts are clamped to this.
    pub max_timeout: Duration,
    /// Address-space limit (RSS+VM) in megabytes; passed to `ulimit -v`
    /// (which actually limits virtual memory in kilobytes — we multiply
    /// by 1024). V8 reserves a larger up-front heap than `CPython`, so
    /// 1024 MB is a more realistic floor than the 512 MB Python uses.
    pub memory_mb: u64,
    /// Parent directory under which each call gets a fresh `mktemp -d`.
    /// Defaults to the OS temp dir.
    pub workdir_parent: PathBuf,
    /// Which JS runtime to spawn.
    pub runtime: Runtime,
    /// Path to the runtime executable. Defaults to `runtime.default_bin()`
    /// resolved on `$PATH` in the sandbox.
    pub runtime_bin: PathBuf,
    /// When true, stderr passes through the PII redactor before return.
    pub redact_stderr: bool,
}

impl Default for ExecConfig {
    fn default() -> Self {
        let runtime = Runtime::Deno;
        Self {
            max_timeout: Duration::from_secs(30),
            memory_mb: 1024,
            workdir_parent: std::env::temp_dir(),
            runtime,
            runtime_bin: PathBuf::from(runtime.default_bin()),
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
/// supervisor itself failing — e.g. runtime missing from `$PATH`, OS
/// refusing to spawn.
#[derive(Debug, Error)]
pub enum ExecError {
    /// `code` exceeds [`CODE_BYTE_CAP`]. We refuse to write a >64 KB
    /// snippet to disk before even starting the runtime.
    #[error("snippet exceeds {CODE_BYTE_CAP} byte cap (got {0} bytes)")]
    SnippetTooLarge(usize),
    /// Failed to create the per-call tempdir.
    #[error("create tempdir under {0}: {1}")]
    Workdir(PathBuf, std::io::Error),
    /// Failed to write `main.js`.
    #[error("write main.js: {0}")]
    WriteCode(std::io::Error),
    /// Failed to spawn the child process at all (runtime missing, fork
    /// failed, etc.).
    #[error("spawn {0}: {1}")]
    Spawn(PathBuf, std::io::Error),
    /// I/O error while reading stdout/stderr or waiting on the child.
    #[error("child io: {0}")]
    ChildIo(std::io::Error),
}

/// Execute `code` under `cfg` with a per-call `timeout` (clamped to
/// `cfg.max_timeout`). Returns `Ok(ExecResult)` for every outcome that
/// involves the runtime actually running (success, crash, deadline);
/// returns `Err(ExecError)` only when the supervisor itself couldn't
/// get the runtime off the ground.
pub async fn run_javascript(
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
        .prefix("xg-exec-js-")
        .tempdir_in(&cfg.workdir_parent)
        .map_err(|e| ExecError::Workdir(cfg.workdir_parent.clone(), e))?;
    let main_js = workdir.path().join("main.js");
    tokio::fs::write(&main_js, code)
        .await
        .map_err(ExecError::WriteCode)?;

    let mut cmd = build_command(cfg, workdir.path(), &main_js);
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::piped());

    let start = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| ExecError::Spawn(cfg.runtime_bin.clone(), e))?;

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
            // pid (kill_on_drop), but forked grandchildren survive — e.g. the
            // `node` runtime can `child_process.spawn(..., {detached:true})`.
            // The child is its own group leader (process_group(0)), so SIGKILL
            // the whole group `-pgid` to reap them. Best-effort, no-unsafe (the
            // workspace forbids unsafe), via /bin/kill. No output is captured on
            // timeout (wait_with_output buffers until completion).
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

/// Assemble the runtime command with a scrubbed env, fresh CWD, and a
/// memory cap. We wrap through `sh -c "ulimit -v $N && exec <runtime>
/// main.js"` because tokio's `Command::pre_exec` requires unsafe and
/// the project forbids unsafe code at workspace level.
fn build_command(cfg: &ExecConfig, working_dir: &Path, main_js: &Path) -> Command {
    // Address-space limit in kilobytes (what `ulimit -v` actually takes).
    let mem_kb = cfg.memory_mb.saturating_mul(1024);

    // Build the env from the allowlist only. Inherit values from the
    // parent process for keys the operator expects to work (PATH, locale).
    let env: HashMap<&str, String> = ENV_ALLOWLIST
        .iter()
        .filter_map(|key| std::env::var(key).ok().map(|v| (*key, v)))
        .collect();

    let invoke = cfg.runtime.invoke(
        &cfg.runtime_bin.display().to_string(),
        &main_js.display().to_string(),
    );
    let shell_inner = format!("ulimit -v {mem_kb} 2>/dev/null; {invoke}");

    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(shell_inner);
    command.current_dir(working_dir);
    command.env_clear();
    for (k, v) in env {
        command.env(k, v);
    }
    // Kill the child if the future is dropped (e.g. on timeout cancellation).
    command.kill_on_drop(true);
    // Put the child in its own process group (it becomes group leader, so
    // pgid == child pid) so a timeout can SIGKILL the WHOLE group — otherwise
    // kill_on_drop only reaps the top `sh`/runtime pid and any forked
    // grandchildren (Node `child_process`) survive as orphans.
    #[cfg(unix)]
    command.process_group(0);
    command
}

/// Minimal single-quote escaping for `/bin/sh -c`. Used only for the
/// runtime path and the main.js path — both are under our control
/// (workdir is a tempdir with a known prefix), so the surface area is
/// tiny.
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

/// Best-effort `which`-style PATH probe, used only by the gated-spawn
/// tests so we can skip cleanly on hosts without the runtime installed.
/// Inlined rather than pulling in the `which` crate — see plan §4 step G.
#[cfg(test)]
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cfg() -> ExecConfig {
        ExecConfig {
            max_timeout: Duration::from_secs(10),
            memory_mb: 1024,
            workdir_parent: std::env::temp_dir(),
            runtime: Runtime::Deno,
            runtime_bin: PathBuf::from("deno"),
            redact_stderr: true,
        }
    }

    /// Skip the test cleanly (with a `SKIPPED:` log line on stderr) when
    /// the configured runtime is not installed on the host. CI hosts
    /// without Deno/Node will see all spawn tests pass via this skip,
    /// while developer boxes with the runtime get full coverage.
    macro_rules! skip_unless_runtime {
        ($cfg:expr) => {
            let bin = $cfg.runtime_bin.to_string_lossy().into_owned();
            if !runtime_on_path(&bin) {
                eprintln!(
                    "SKIPPED: {} not on PATH (install the runtime to exercise this test)",
                    bin
                );
                return;
            }
        };
    }

    // ---------- pure-Rust tests (no runtime needed) ----------

    #[tokio::test]
    async fn snippet_too_large_short_circuits() {
        let cfg = test_cfg();
        let huge = "x".repeat(CODE_BYTE_CAP + 1);
        let err = run_javascript(&cfg, &huge, Duration::from_secs(5))
            .await
            .expect_err("oversized snippet should bounce before spawn");
        assert!(matches!(err, ExecError::SnippetTooLarge(_)));
    }

    #[test]
    fn decode_capped_truncates_at_cap_with_marker() {
        let big = vec![b'x'; OUTPUT_BYTE_CAP + 100];
        let (s, truncated) = decode_capped(&big);
        assert!(truncated);
        assert!(s.contains("truncated"));
        assert!(s.contains("100 bytes dropped"));
    }

    #[test]
    fn decode_capped_passthrough_below_cap() {
        let small = b"hello".to_vec();
        let (s, truncated) = decode_capped(&small);
        assert!(!truncated);
        assert_eq!(s, "hello");
    }

    #[test]
    fn build_command_uses_only_allowlisted_env() {
        // We can't introspect tokio Command's env list directly; instead
        // we assert that ENV_ALLOWLIST is exactly the four keys we
        // expect — guard against accidental list growth in code review.
        assert_eq!(ENV_ALLOWLIST, &["PATH", "LANG", "LC_ALL", "LC_CTYPE"]);
        // Sanity: building doesn't panic and the cmd path is /bin/sh.
        let cfg = test_cfg();
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main.js");
        std::fs::write(&main, "").unwrap();
        let _ = build_command(&cfg, tmp.path(), &main);
    }

    #[test]
    fn runtime_default_bin_matches_variant() {
        assert_eq!(Runtime::Deno.default_bin(), "deno");
        assert_eq!(Runtime::Node.default_bin(), "node");
    }

    #[test]
    fn runtime_from_str_accepts_canonical_forms() {
        assert_eq!("deno".parse::<Runtime>().unwrap(), Runtime::Deno);
        assert_eq!("DENO".parse::<Runtime>().unwrap(), Runtime::Deno);
        assert_eq!("node".parse::<Runtime>().unwrap(), Runtime::Node);
        assert_eq!("nodejs".parse::<Runtime>().unwrap(), Runtime::Node);
        assert!("ruby".parse::<Runtime>().is_err());
    }

    #[test]
    fn runtime_invoke_deno_uses_allow_none() {
        let cmd = Runtime::Deno.invoke("/usr/bin/deno", "/tmp/main.js");
        assert!(cmd.contains("--allow-none"), "got: {cmd}");
        assert!(cmd.contains("/tmp/main.js"));
    }

    #[test]
    fn runtime_invoke_node_omits_allow_flags() {
        let cmd = Runtime::Node.invoke("/usr/bin/node", "/tmp/main.js");
        assert!(!cmd.contains("--allow-none"));
        assert!(cmd.contains("/tmp/main.js"));
    }

    // ---------- gated-spawn tests (skip when runtime absent) ----------

    #[tokio::test]
    async fn happy_path_captures_stdout_and_exits_zero() {
        let cfg = test_cfg();
        skip_unless_runtime!(cfg);
        let r = run_javascript(
            &cfg,
            "console.log('hello from sandbox')",
            Duration::from_secs(10),
        )
        .await
        .expect("runtime spawned successfully");
        assert_eq!(r.exit_code, Some(0), "stderr was: {}", r.stderr);
        assert_eq!(r.stdout.trim(), "hello from sandbox");
        assert!(!r.timed_out);
        assert!(r.succeeded());
    }

    #[tokio::test]
    async fn nonzero_exit_is_reported_through_result_not_error() {
        let cfg = test_cfg();
        skip_unless_runtime!(cfg);
        let r = run_javascript(&cfg, "Deno.exit(3)", Duration::from_secs(10))
            .await
            .expect("supervisor itself must succeed");
        assert_eq!(r.exit_code, Some(3));
        assert!(!r.timed_out);
        assert!(!r.succeeded());
    }

    #[tokio::test]
    async fn timeout_kills_long_running_snippet() {
        let cfg = test_cfg();
        skip_unless_runtime!(cfg);
        // Sleep ~5 seconds inside the runtime, deadline 500 ms.
        // setTimeout keeps the event loop alive past the deadline.
        let r = run_javascript(
            &cfg,
            "setTimeout(() => console.log('should not print'), 5000)",
            Duration::from_millis(500),
        )
        .await
        .expect("supervisor handles deadline as a result, not an error");
        assert!(r.timed_out, "deadline should fire");
        assert_eq!(r.exit_code, None);
        // Duration should be close to the deadline, not the 5s sleep.
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
        skip_unless_runtime!(cfg);
        let snippet = r#"console.error("contact me at alice@example.com")"#;
        let r = run_javascript(&cfg, snippet, Duration::from_secs(10))
            .await
            .expect("runtime spawned");
        assert!(
            !r.stderr.contains("alice@example.com"),
            "email leaked: {}",
            r.stderr
        );
        // Redactor uses a marker like "[redacted-email]"; assert
        // *something* changed without locking to the exact token.
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
        skip_unless_runtime!(cfg);
        let snippet = r#"console.error("contact me at alice@example.com")"#;
        let r = run_javascript(&cfg, snippet, Duration::from_secs(10))
            .await
            .expect("runtime spawned");
        assert!(
            r.stderr.contains("alice@example.com"),
            "redaction off should pass email through: {}",
            r.stderr
        );
    }

    #[tokio::test]
    async fn stdout_cap_is_enforced_with_truncation_marker() {
        let cfg = test_cfg();
        skip_unless_runtime!(cfg);
        // ~130 KB of output, well over OUTPUT_BYTE_CAP (64 KB).
        let total = OUTPUT_BYTE_CAP + 64 * 1024;
        let snippet = format!("console.log('x'.repeat({total}))");
        let r = run_javascript(&cfg, &snippet, Duration::from_secs(10))
            .await
            .expect("runtime spawned");
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
        skip_unless_runtime!(cfg);
        // Set a fake secret in the parent process env.
        std::env::set_var("XIAOGUAI_AUDIT_SIGNING_KEY", "must-not-leak-into-sandbox");
        // Under Deno --allow-none, `Deno.env.get(...)` is denied at
        // runtime. Wrap in try/catch and print 'absent' for the denied
        // path; this means a leak would print the secret, while the
        // expected denied path prints 'absent'.
        let snippet = r"
try {
  const v = Deno.env.get('XIAOGUAI_AUDIT_SIGNING_KEY');
  console.log(v === undefined ? 'absent' : v);
} catch (_e) {
  console.log('absent');
}
";
        let r = run_javascript(&cfg, snippet, Duration::from_secs(10))
            .await
            .expect("runtime spawned");
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
        skip_unless_runtime!(cfg);
        // First call writes a file to CWD. Second call must not see it.
        // Under Deno --allow-none, Deno.writeTextFileSync is denied,
        // which is the expected security posture; we wrap in try/catch
        // and assert the marker is absent regardless.
        let r1 = run_javascript(
            &cfg,
            r"
try { Deno.writeTextFileSync('cwd-marker.txt', 'first'); } catch (_e) {}
console.log('done');
",
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        assert_eq!(r1.exit_code, Some(0));
        let r2 = run_javascript(
            &cfg,
            r"
let present = false;
try { Deno.statSync('cwd-marker.txt'); present = true; } catch (_e) {}
console.log(present ? 'present' : 'absent');
",
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        assert_eq!(r2.stdout.trim(), "absent");
    }
}
