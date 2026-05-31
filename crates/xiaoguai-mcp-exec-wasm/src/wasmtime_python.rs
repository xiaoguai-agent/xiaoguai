//! `WasmtimePythonBackend` — L3 Python sandbox via wasmtime + pyodide.
//!
//! The module ABI we target: a WASI command module that reads its
//! script from a preopened `/snippet.py` and writes stdout/stderr
//! through standard WASI streams. This matches the layout of
//! `python-wasi` (`python.wasm` from <https://github.com/singlestore-labs/python-wasi>)
//! and the operator-side `scripts/fetch-wasm-assets.sh` produces such a
//! blob.
//!
//! Per-call lifecycle:
//! 1. Compose a `wasmtime::Store` with a `StoreLimits` capping memory
//!    at `cfg.memory_mb`.
//! 2. Set the store's epoch deadline to `ticks_for_secs(timeout)` and
//!    trap mode = `Store::epoch_deadline_trap`.
//! 3. Build a `WasiCtx` with **no env**, no preopened host directories
//!    (the snippet lives in an in-memory file via the wasi-virt path),
//!    stdout/stderr piped to memory.
//! 4. Instantiate the cached module, call `_start` (the WASI command
//!    entry point) asynchronously so the epoch interruption can fire.
//! 5. Drain the memory pipes, decode + truncate + redact, build an
//!    [`ExecResult`].

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use thiserror::Error;
use tracing::{debug, warn};
use wasmtime::{Module, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::p1::{self, WasiP1Ctx};
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};
use wasmtime_wasi::WasiCtxBuilder;
use xiaoguai_mcp_exec::runtime::{CapabilitySummary, ExecBackend};
use xiaoguai_mcp_exec::{ExecError, ExecResult};
use xiaoguai_types::redact::redact_str;

use crate::assets::{load_pyodide_module, AssetError};
use crate::config::WasmExecConfig;
use crate::engine::{shared_engine, ticks_for_secs};

/// Hard cap on captured output per stream (matches L1).
const OUTPUT_BYTE_CAP: usize = 64 * 1024;
/// Hard cap on the snippet (matches L1).
const CODE_BYTE_CAP: usize = 64 * 1024;

/// Failure paths unique to the L3 backend; L1 errors (snippet too
/// large, child IO) are re-used via [`ExecError`] for caller parity.
#[derive(Debug, Error)]
pub enum WasmBackendError {
    /// WASM asset missing or unloadable. Carries the user-facing
    /// installation hint from `AssetError`.
    #[error(transparent)]
    Asset(#[from] AssetError),
    /// Wasmtime returned an instantiation, linking, or trap error that
    /// isn't a timeout (timeouts surface as `ExecResult.timed_out`).
    #[error("wasmtime: {0}")]
    Wasmtime(String),
}

/// L3 Python backend. Holds the per-instance config and a lazily
/// compiled module cached in a `OnceLock`.
pub struct WasmtimePythonBackend {
    cfg: WasmExecConfig,
    module: OnceLock<Result<Module, AssetError>>,
}

impl std::fmt::Debug for WasmtimePythonBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmtimePythonBackend")
            .field("cfg", &self.cfg)
            .field("module_cached", &self.module.get().is_some())
            .finish()
    }
}

impl WasmtimePythonBackend {
    /// Construct without eagerly loading the module. The first call to
    /// [`ExecBackend::run`] (or [`Self::ensure_module`]) triggers the
    /// load.
    #[must_use]
    pub fn new(cfg: WasmExecConfig) -> Self {
        Self {
            cfg,
            module: OnceLock::new(),
        }
    }

    /// Force-load the module. Returns the cached `AssetError` if the
    /// load failed. Useful at boot to fail fast instead of on the
    /// first request.
    pub fn ensure_module(&self) -> Result<&Module, &AssetError> {
        let entry = self.module.get_or_init(|| {
            let override_path = self.cfg.module_path.as_deref();
            load_pyodide_module(shared_engine(), override_path)
        });
        entry.as_ref()
    }

    /// Borrow the underlying config.
    #[must_use]
    pub fn config(&self) -> &WasmExecConfig {
        &self.cfg
    }
}

#[async_trait]
impl ExecBackend for WasmtimePythonBackend {
    fn name(&self) -> &'static str {
        "wasmtime-l3-python"
    }

    async fn run(&self, snippet: &str, timeout: Duration) -> Result<ExecResult, ExecError> {
        if snippet.len() > CODE_BYTE_CAP {
            return Err(ExecError::SnippetTooLarge(snippet.len()));
        }
        let deadline = std::cmp::min(timeout, self.cfg.max_timeout);

        let module = match self.ensure_module() {
            Ok(m) => m.clone(),
            Err(asset_err) => {
                // Surface as a "spawn-equivalent" supervisor error so
                // the MCP layer raises `is_error=true` rather than
                // pretending the snippet just exited non-zero.
                return Err(ExecError::Spawn(
                    PathBuf::from("pyodide.wasm"),
                    std::io::Error::other(asset_err.to_string()),
                ));
            }
        };

        let start = Instant::now();
        let result = run_wasi_snippet(
            &module,
            snippet,
            deadline,
            self.cfg.memory_mb,
            self.cfg.redact_stderr,
        )
        .await;
        let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

        match result {
            Ok((exit_code, stdout, stderr, truncated)) => Ok(ExecResult {
                exit_code,
                stdout,
                stderr,
                duration_ms,
                truncated,
                timed_out: false,
            }),
            Err(WasiRunError::Timeout) => Ok(ExecResult {
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                duration_ms,
                truncated: false,
                timed_out: true,
            }),
            Err(WasiRunError::Wasmtime(s)) => {
                warn!(error = %s, "wasmtime backend error");
                Err(ExecError::ChildIo(std::io::Error::other(s)))
            }
        }
    }

    fn capability_summary(&self) -> CapabilitySummary {
        CapabilitySummary {
            tier: "L3",
            language: "python",
            network: false,
            // No preopens, no host fs.
            filesystem: false,
            // WASM cannot spawn.
            subprocess: false,
            max_memory_mb: self.cfg.memory_mb,
            max_timeout_secs: self.cfg.max_timeout.as_secs(),
        }
    }
}

/// Per-call wasmtime store carrying both the WASI context and the
/// memory limiter. Lifetime ends when the call returns.
struct StoreState {
    wasi: WasiP1Ctx,
    limits: StoreLimits,
}

/// Internal error type — distinguishes timeout (which the public API
/// reports as `timed_out`) from genuine wasmtime errors (which the
/// public API surfaces as `ExecError::ChildIo`).
enum WasiRunError {
    Timeout,
    Wasmtime(String),
}

/// Compose a `WasiCtx`, build a Store, instantiate the module, call
/// `_start`, and harvest the memory pipes. The snippet is exposed to
/// the module by writing it to a virtual file at `/snippet.py` via
/// the WASI preview1 in-memory pipe — but **without** preopening any
/// host directory.
async fn run_wasi_snippet(
    module: &Module,
    snippet: &str,
    deadline: Duration,
    memory_mb: u64,
    redact_stderr: bool,
) -> Result<(Option<i32>, String, String, bool), WasiRunError> {
    let engine = shared_engine();

    let stdout_pipe = MemoryOutputPipe::new(OUTPUT_BYTE_CAP * 2);
    let stderr_pipe = MemoryOutputPipe::new(OUTPUT_BYTE_CAP * 2);

    // WASI context with NO env, NO preopens. The snippet is delivered
    // via stdin (a memory pipe) — pyodide's WASI build reads its
    // script from stdin, matching common `python -` semantics.
    let mut wasi_builder = WasiCtxBuilder::new();
    wasi_builder
        .stdin(MemoryInputPipe::new(snippet.as_bytes().to_vec()))
        .stdout(stdout_pipe.clone())
        .stderr(stderr_pipe.clone());
    let wasi = wasi_builder.build_p1();

    let mem_bytes = (memory_mb as usize).saturating_mul(1024 * 1024);
    let limits = StoreLimitsBuilder::new()
        .memory_size(mem_bytes)
        // Single instance per call.
        .instances(1)
        .build();

    let state = StoreState { wasi, limits };
    let mut store: Store<StoreState> = Store::new(engine, state);
    store.limiter(|s| &mut s.limits);

    // Epoch deadline: convert timeout into ticks. Trap (not yield) on
    // expiry — the snippet has had its turn.
    let ticks = ticks_for_secs(deadline.as_secs().max(1));
    store.set_epoch_deadline(ticks);
    store.epoch_deadline_trap();

    let mut linker = wasmtime::Linker::<StoreState>::new(engine);
    p1::add_to_linker_async(&mut linker, |s| &mut s.wasi)
        .map_err(|e| WasiRunError::Wasmtime(format!("wasi link: {e}")))?;

    let instance = linker
        .instantiate_async(&mut store, module)
        .await
        .map_err(|e| WasiRunError::Wasmtime(format!("instantiate: {e}")))?;

    // WASI command entry point. Pyodide / python-wasi expose `_start`.
    let start_fn = instance
        .get_typed_func::<(), ()>(&mut store, "_start")
        .map_err(|e| WasiRunError::Wasmtime(format!("get _start: {e}")))?;

    let call_result = start_fn.call_async(&mut store, ()).await;

    // Decode pipes regardless of how _start returned — partial output
    // is still useful diagnostically.
    let stdout_bytes = stdout_pipe.contents().to_vec();
    let stderr_bytes = stderr_pipe.contents().to_vec();
    let (stdout, stdout_trunc) = decode_capped(&stdout_bytes);
    let (stderr_text, stderr_trunc) = decode_capped(&stderr_bytes);
    let stderr = if redact_stderr {
        redact_str(&stderr_text)
    } else {
        stderr_text
    };
    let truncated = stdout_trunc || stderr_trunc;

    match call_result {
        Ok(()) => {
            // WASI `_start` returns `()` on graceful exit (exit code 0).
            // Non-zero exit is reported via `proc_exit` which appears
            // as a trap; matched below.
            Ok((Some(0), stdout, stderr, truncated))
        }
        Err(trap) => {
            let msg = trap.to_string();
            // Epoch interruption surface (trap message contains
            // "interrupt").
            if msg.contains("interrupt") || msg.contains("epoch") {
                debug!("snippet hit epoch deadline");
                return Err(WasiRunError::Timeout);
            }
            // WASI `proc_exit(code)` is reported as a trap with the
            // code embedded. Extract it if we can; fall back to
            // exit=None.
            if let Some(exit) = extract_proc_exit_code(&msg) {
                return Ok((Some(exit), stdout, stderr, truncated));
            }
            // OOM (memory grow refused by limiter) surfaces as a trap.
            // Surface to caller as a runtime error so HotL can
            // distinguish from successful non-zero exit.
            Err(WasiRunError::Wasmtime(msg))
        }
    }
}

/// Best-effort parse of a `proc_exit(code)` trap message into a
/// process-style exit code. Wasmtime's exact format may change across
/// versions, so we look for both `"exit code: N"` and the raw integer
/// near the end.
fn extract_proc_exit_code(msg: &str) -> Option<i32> {
    // Newer wasmtime uses `"Exited with i32 exit status N"`.
    for needle in ["exit status ", "exit code: ", "exit code "] {
        if let Some(pos) = msg.find(needle) {
            let tail = &msg[pos + needle.len()..];
            let num: String = tail.chars().take_while(char::is_ascii_digit).collect();
            if let Ok(code) = num.parse::<i32>() {
                return Some(code);
            }
        }
    }
    None
}

/// Decode bytes lossily, capped at [`OUTPUT_BYTE_CAP`]. Same shape as
/// the L1 helper so output looks identical to operators flipping
/// tiers.
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
    use std::path::Path;

    /// Skip the test cleanly if the pyodide asset is not on disk.
    /// Mirror of the L1 JS `skip_unless_runtime!` macro.
    macro_rules! skip_unless_assets {
        () => {
            if std::env::var_os(crate::assets::PYODIDE_PATH_ENV).is_none() {
                eprintln!(
                    "SKIPPED: {} not set (provision pyodide.wasm via scripts/fetch-wasm-assets.sh)",
                    crate::assets::PYODIDE_PATH_ENV
                );
                return;
            }
        };
    }

    fn cfg_with(memory_mb: u64, timeout_secs: u64) -> WasmExecConfig {
        WasmExecConfig {
            max_timeout: Duration::from_secs(timeout_secs),
            memory_mb,
            redact_stderr: true,
            module_path: None,
        }
    }

    // ---------- unconditional tests (no asset needed) ----------

    #[tokio::test]
    async fn snippet_too_large_rejected() {
        let backend = WasmtimePythonBackend::new(cfg_with(256, 30));
        let huge = "x".repeat(CODE_BYTE_CAP + 1);
        let err = backend
            .run(&huge, Duration::from_secs(5))
            .await
            .expect_err("oversized snippet must be rejected before any module load");
        assert!(matches!(err, ExecError::SnippetTooLarge(_)));
    }

    #[test]
    fn capability_summary_reports_l3_python() {
        let backend = WasmtimePythonBackend::new(cfg_with(256, 30));
        let cap = backend.capability_summary();
        assert_eq!(cap.tier, "L3");
        assert_eq!(cap.language, "python");
        assert!(!cap.network);
        assert!(!cap.filesystem, "L3 must have no host fs");
        assert!(!cap.subprocess, "L3 must have no subprocess");
        assert_eq!(cap.max_memory_mb, 256);
        assert_eq!(cap.max_timeout_secs, 30);
    }

    #[test]
    fn name_is_stable() {
        let backend = WasmtimePythonBackend::new(WasmExecConfig::default());
        assert_eq!(backend.name(), "wasmtime-l3-python");
    }

    #[tokio::test]
    async fn asset_missing_surfaces_as_supervisor_error() {
        // Ensure env var is unset for this test.
        let prev = std::env::var_os(crate::assets::PYODIDE_PATH_ENV);
        std::env::remove_var(crate::assets::PYODIDE_PATH_ENV);

        let backend = WasmtimePythonBackend::new(cfg_with(256, 5));
        let err = backend
            .run("print('hi')", Duration::from_secs(2))
            .await
            .expect_err("missing asset must surface");

        // Restore env.
        if let Some(v) = prev {
            std::env::set_var(crate::assets::PYODIDE_PATH_ENV, v);
        }

        match err {
            ExecError::Spawn(path, io_err) => {
                assert_eq!(path, PathBuf::from("pyodide.wasm"));
                let msg = io_err.to_string();
                assert!(
                    msg.contains("XIAOGUAI_PYODIDE_PATH"),
                    "missing env hint: {msg}"
                );
            }
            other => panic!("expected Spawn(pyodide.wasm), got {other:?}"),
        }
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
        let small = b"hi".to_vec();
        let (s, truncated) = decode_capped(&small);
        assert!(!truncated);
        assert_eq!(s, "hi");
    }

    #[test]
    fn extract_proc_exit_code_parses_wasmtime_format() {
        let msg = "Exited with i32 exit status 3";
        assert_eq!(extract_proc_exit_code(msg), Some(3));
        let msg2 = "wasm trap: exit code: 7";
        assert_eq!(extract_proc_exit_code(msg2), Some(7));
        let msg3 = "ordinary trap with no code";
        assert_eq!(extract_proc_exit_code(msg3), None);
    }

    #[test]
    fn ensure_module_caches_on_failure_too() {
        let prev = std::env::var_os(crate::assets::PYODIDE_PATH_ENV);
        std::env::remove_var(crate::assets::PYODIDE_PATH_ENV);
        let backend = WasmtimePythonBackend::new(cfg_with(256, 5));
        // Two calls — both should hit the cached AssetMissing without
        // racing.
        assert!(backend.ensure_module().is_err());
        assert!(backend.ensure_module().is_err());
        if let Some(v) = prev {
            std::env::set_var(crate::assets::PYODIDE_PATH_ENV, v);
        }
    }

    #[test]
    fn debug_impl_does_not_panic_uninstantiated() {
        let backend = WasmtimePythonBackend::new(WasmExecConfig::default());
        let _ = format!("{backend:?}");
    }

    // ---------- gated-asset tests (skip when pyodide.wasm absent) ----------

    #[tokio::test]
    async fn happy_path_print_hello() {
        skip_unless_assets!();
        let backend = WasmtimePythonBackend::new(WasmExecConfig::default());
        let r = backend
            .run("print('hello from L3')", Duration::from_secs(10))
            .await
            .expect("pyodide WASI happy path");
        assert!(r.stdout.contains("hello from L3"), "stdout: {}", r.stdout);
        assert_eq!(r.exit_code, Some(0));
        assert!(!r.timed_out);
    }

    #[tokio::test]
    async fn nonzero_exit_reported_through_result() {
        skip_unless_assets!();
        let backend = WasmtimePythonBackend::new(WasmExecConfig::default());
        let r = backend
            .run("import sys; sys.exit(5)", Duration::from_secs(10))
            .await
            .expect("supervisor itself succeeds even on snippet failure");
        assert_eq!(r.exit_code, Some(5));
        assert!(!r.timed_out);
    }

    #[tokio::test]
    async fn timeout_via_epoch_kills_long_loop() {
        skip_unless_assets!();
        let backend = WasmtimePythonBackend::new(WasmExecConfig {
            max_timeout: Duration::from_secs(1),
            ..WasmExecConfig::default()
        });
        let r = backend
            .run("while True: pass", Duration::from_millis(500))
            .await
            .expect("epoch trap reported as timed_out");
        assert!(r.timed_out, "epoch deadline should fire");
        assert_eq!(r.exit_code, None);
    }

    #[tokio::test]
    async fn env_not_visible_in_sandbox() {
        skip_unless_assets!();
        std::env::set_var("XG_TEST_LEAK_KEY", "secret");
        let backend = WasmtimePythonBackend::new(WasmExecConfig::default());
        let r = backend
            .run(
                "import os; print(os.environ.get('XG_TEST_LEAK_KEY', 'absent'))",
                Duration::from_secs(10),
            )
            .await
            .expect("WASI run");
        std::env::remove_var("XG_TEST_LEAK_KEY");
        assert_eq!(
            r.stdout.trim(),
            "absent",
            "L3 env scrub failed: {}",
            r.stdout
        );
    }

    #[tokio::test]
    async fn stdout_cap_truncates_at_64kb() {
        skip_unless_assets!();
        let backend = WasmtimePythonBackend::new(WasmExecConfig::default());
        let snippet = format!("print('x' * {})", OUTPUT_BYTE_CAP + 32 * 1024);
        let r = backend
            .run(&snippet, Duration::from_secs(10))
            .await
            .expect("WASI run");
        assert!(r.truncated, "expected truncation marker");
        assert!(r.stdout.contains("truncated"));
    }

    #[tokio::test]
    async fn stderr_redaction_applies_to_email() {
        skip_unless_assets!();
        let backend = WasmtimePythonBackend::new(WasmExecConfig::default());
        let snippet = r#"
import sys
print("contact: alice@example.com", file=sys.stderr)
"#;
        let r = backend
            .run(snippet, Duration::from_secs(10))
            .await
            .expect("WASI run");
        assert!(
            !r.stderr.contains("alice@example.com"),
            "L3 redactor failed: {}",
            r.stderr
        );
    }

    #[tokio::test]
    async fn cold_start_under_200ms() {
        skip_unless_assets!();
        let backend = WasmtimePythonBackend::new(WasmExecConfig::default());
        // First call warms the module cache; we measure the second
        // call, which is the per-invocation cost the user pays.
        let _warm = backend.run("pass", Duration::from_secs(10)).await;
        let start = Instant::now();
        let _ = backend.run("pass", Duration::from_secs(10)).await;
        let elapsed = start.elapsed();
        // Soft target 50 ms (ADR-0020 "cached path"); hard fail 200 ms.
        #[cfg(not(debug_assertions))]
        assert!(
            elapsed < Duration::from_millis(200),
            "cold-start cached-path budget blown: {:?}",
            elapsed
        );
        // In debug builds we don't enforce the budget but log it.
        eprintln!("L3 python cached cold start: {elapsed:?}");
    }

    #[tokio::test]
    async fn concurrent_calls_do_not_share_state() {
        skip_unless_assets!();
        let backend = std::sync::Arc::new(WasmtimePythonBackend::new(WasmExecConfig::default()));
        let b1 = backend.clone();
        let b2 = backend.clone();
        let h1 =
            tokio::spawn(async move { b1.run("g = 'A'; print(g)", Duration::from_secs(10)).await });
        let h2 =
            tokio::spawn(async move { b2.run("g = 'B'; print(g)", Duration::from_secs(10)).await });
        let r1 = h1.await.unwrap().unwrap();
        let r2 = h2.await.unwrap().unwrap();
        // Each interpreter is its own Store; global `g` is independent.
        assert!(r1.stdout.contains('A'));
        assert!(r2.stdout.contains('B'));
    }

    // Sanity check that Path import is still used (silences unused
    // warning if any code path is feature-gated away).
    #[allow(dead_code)]
    fn _path_usage(p: &Path) -> bool {
        p.is_file()
    }
}
