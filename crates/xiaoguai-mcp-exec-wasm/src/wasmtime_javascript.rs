//! `WasmtimeJavaScriptBackend` — L3 JavaScript sandbox via wasmtime +
//! QuickJS-WASM.
//!
//! Mirrors [`crate::wasmtime_python`] step-for-step: same engine, same
//! epoch-deadline mechanism, same memory limiter, same output capture
//! pipeline. Differences:
//!
//! - Module is loaded from `XIAOGUAI_QUICKJS_PATH` (or
//!   `config.module_path`).
//! - The QuickJS-WASI build (from
//!   <https://github.com/saghul/quickjs-emscripten>) accepts the script
//!   on stdin, just like `python -`.
//! - Backend name in metrics is `"wasmtime-l3-javascript"`.
//! - Surfaced ExecError on asset miss is keyed to `quickjs.wasm` so the
//!   operator hint points at the right env var.

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
// The L3 JS backend uses the *Python* crate's L1 trait + error types
// because that's where the canonical `CapabilitySummary`/`ExecBackend`
// live. The JS crate has its own structurally-identical pair. Tests
// in this module exercise the python-crate trait; downstream callers
// that need the JS-crate trait can wrap via the same shape.
use xiaoguai_mcp_exec::runtime::{CapabilitySummary, ExecBackend};
use xiaoguai_mcp_exec::{ExecError, ExecResult};
use xiaoguai_types::redact::redact_str;

use crate::assets::{load_quickjs_module, AssetError};
use crate::config::WasmExecConfig;
use crate::engine::{shared_engine, ticks_for_secs};

const OUTPUT_BYTE_CAP: usize = 64 * 1024;
const CODE_BYTE_CAP: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum WasmBackendError {
    #[error(transparent)]
    Asset(#[from] AssetError),
    #[error("wasmtime: {0}")]
    Wasmtime(String),
}

pub struct WasmtimeJavaScriptBackend {
    cfg: WasmExecConfig,
    module: OnceLock<Result<Module, AssetError>>,
}

impl std::fmt::Debug for WasmtimeJavaScriptBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmtimeJavaScriptBackend")
            .field("cfg", &self.cfg)
            .field("module_cached", &self.module.get().is_some())
            .finish()
    }
}

impl WasmtimeJavaScriptBackend {
    #[must_use]
    pub fn new(cfg: WasmExecConfig) -> Self {
        Self {
            cfg,
            module: OnceLock::new(),
        }
    }

    /// Force-load the QuickJS module; same semantics as the Python
    /// backend's `ensure_module`.
    pub fn ensure_module(&self) -> Result<&Module, &AssetError> {
        let entry = self.module.get_or_init(|| {
            let override_path = self.cfg.module_path.as_deref();
            load_quickjs_module(shared_engine(), override_path)
        });
        entry.as_ref()
    }

    #[must_use]
    pub fn config(&self) -> &WasmExecConfig {
        &self.cfg
    }
}

#[async_trait]
impl ExecBackend for WasmtimeJavaScriptBackend {
    fn name(&self) -> &'static str {
        "wasmtime-l3-javascript"
    }

    async fn run(&self, snippet: &str, timeout: Duration) -> Result<ExecResult, ExecError> {
        if snippet.len() > CODE_BYTE_CAP {
            return Err(ExecError::SnippetTooLarge(snippet.len()));
        }
        let deadline = std::cmp::min(timeout, self.cfg.max_timeout);

        let module = match self.ensure_module() {
            Ok(m) => m.clone(),
            Err(asset_err) => {
                return Err(ExecError::Spawn(
                    PathBuf::from("quickjs.wasm"),
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
                warn!(error = %s, "wasmtime JS backend error");
                Err(ExecError::ChildIo(std::io::Error::other(s)))
            }
        }
    }

    fn capability_summary(&self) -> CapabilitySummary {
        CapabilitySummary {
            tier: "L3",
            language: "javascript",
            network: false,
            filesystem: false,
            subprocess: false,
            max_memory_mb: self.cfg.memory_mb,
            max_timeout_secs: self.cfg.max_timeout.as_secs(),
        }
    }
}

struct StoreState {
    wasi: WasiP1Ctx,
    limits: StoreLimits,
}

enum WasiRunError {
    Timeout,
    Wasmtime(String),
}

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

    let mut wasi_builder = WasiCtxBuilder::new();
    wasi_builder
        .stdin(MemoryInputPipe::new(snippet.as_bytes().to_vec()))
        .stdout(stdout_pipe.clone())
        .stderr(stderr_pipe.clone());
    let wasi = wasi_builder.build_p1();

    let mem_bytes = (memory_mb as usize).saturating_mul(1024 * 1024);
    let limits = StoreLimitsBuilder::new()
        .memory_size(mem_bytes)
        .instances(1)
        .build();

    let state = StoreState { wasi, limits };
    let mut store: Store<StoreState> = Store::new(engine, state);
    store.limiter(|s| &mut s.limits);

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

    let start_fn = instance
        .get_typed_func::<(), ()>(&mut store, "_start")
        .map_err(|e| WasiRunError::Wasmtime(format!("get _start: {e}")))?;

    let call_result = start_fn.call_async(&mut store, ()).await;

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
        Ok(()) => Ok((Some(0), stdout, stderr, truncated)),
        Err(trap) => {
            let msg = trap.to_string();
            if msg.contains("interrupt") || msg.contains("epoch") {
                debug!("JS snippet hit epoch deadline");
                return Err(WasiRunError::Timeout);
            }
            if let Some(exit) = extract_proc_exit_code(&msg) {
                return Ok((Some(exit), stdout, stderr, truncated));
            }
            Err(WasiRunError::Wasmtime(msg))
        }
    }
}

fn extract_proc_exit_code(msg: &str) -> Option<i32> {
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

    macro_rules! skip_unless_assets {
        () => {
            if std::env::var_os(crate::assets::QUICKJS_PATH_ENV).is_none() {
                eprintln!(
                    "SKIPPED: {} not set (provision quickjs.wasm via scripts/fetch-wasm-assets.sh)",
                    crate::assets::QUICKJS_PATH_ENV
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

    // ---------- unconditional tests ----------

    #[tokio::test]
    async fn snippet_too_large_rejected() {
        let backend = WasmtimeJavaScriptBackend::new(cfg_with(256, 30));
        let huge = "x".repeat(CODE_BYTE_CAP + 1);
        let err = backend
            .run(&huge, Duration::from_secs(5))
            .await
            .expect_err("oversized snippet rejected before module load");
        assert!(matches!(err, ExecError::SnippetTooLarge(_)));
    }

    #[test]
    fn capability_summary_reports_l3_javascript() {
        let backend = WasmtimeJavaScriptBackend::new(cfg_with(256, 30));
        let cap = backend.capability_summary();
        assert_eq!(cap.tier, "L3");
        assert_eq!(cap.language, "javascript");
        assert!(!cap.network);
        assert!(!cap.filesystem);
        assert!(!cap.subprocess);
        assert_eq!(cap.max_memory_mb, 256);
        assert_eq!(cap.max_timeout_secs, 30);
    }

    #[test]
    fn name_is_stable() {
        let backend = WasmtimeJavaScriptBackend::new(WasmExecConfig::default());
        assert_eq!(backend.name(), "wasmtime-l3-javascript");
    }

    #[tokio::test]
    async fn asset_missing_surfaces_as_supervisor_error() {
        let prev = std::env::var_os(crate::assets::QUICKJS_PATH_ENV);
        std::env::remove_var(crate::assets::QUICKJS_PATH_ENV);

        let backend = WasmtimeJavaScriptBackend::new(cfg_with(256, 5));
        let err = backend
            .run("console.log('hi')", Duration::from_secs(2))
            .await
            .expect_err("missing asset must surface");

        if let Some(v) = prev {
            std::env::set_var(crate::assets::QUICKJS_PATH_ENV, v);
        }

        match err {
            ExecError::Spawn(path, io_err) => {
                assert_eq!(path, PathBuf::from("quickjs.wasm"));
                let msg = io_err.to_string();
                assert!(
                    msg.contains("XIAOGUAI_QUICKJS_PATH"),
                    "missing env hint: {msg}"
                );
            }
            other => panic!("expected Spawn(quickjs.wasm), got {other:?}"),
        }
    }

    #[test]
    fn decode_capped_truncates_at_cap_with_marker() {
        let big = vec![b'x'; OUTPUT_BYTE_CAP + 200];
        let (s, truncated) = decode_capped(&big);
        assert!(truncated);
        assert!(s.contains("truncated"));
        assert!(s.contains("200 bytes dropped"));
    }

    #[test]
    fn decode_capped_passthrough_below_cap() {
        let small = b"ok".to_vec();
        let (s, truncated) = decode_capped(&small);
        assert!(!truncated);
        assert_eq!(s, "ok");
    }

    #[test]
    fn extract_proc_exit_code_parses_wasmtime_format() {
        let msg = "Exited with i32 exit status 9";
        assert_eq!(extract_proc_exit_code(msg), Some(9));
    }

    #[test]
    fn ensure_module_caches_on_failure_too() {
        let prev = std::env::var_os(crate::assets::QUICKJS_PATH_ENV);
        std::env::remove_var(crate::assets::QUICKJS_PATH_ENV);
        let backend = WasmtimeJavaScriptBackend::new(cfg_with(256, 5));
        assert!(backend.ensure_module().is_err());
        assert!(backend.ensure_module().is_err());
        if let Some(v) = prev {
            std::env::set_var(crate::assets::QUICKJS_PATH_ENV, v);
        }
    }

    #[test]
    fn debug_impl_does_not_panic_uninstantiated() {
        let backend = WasmtimeJavaScriptBackend::new(WasmExecConfig::default());
        let _ = format!("{backend:?}");
    }

    // ---------- gated-asset tests ----------

    #[tokio::test]
    async fn happy_path_console_log() {
        skip_unless_assets!();
        let backend = WasmtimeJavaScriptBackend::new(WasmExecConfig::default());
        let r = backend
            .run("console.log('hello from JS L3')", Duration::from_secs(10))
            .await
            .expect("QuickJS WASI happy path");
        assert!(
            r.stdout.contains("hello from JS L3"),
            "stdout: {}",
            r.stdout
        );
        assert_eq!(r.exit_code, Some(0));
        assert!(!r.timed_out);
    }

    #[tokio::test]
    async fn nonzero_exit_reported_through_result() {
        skip_unless_assets!();
        let backend = WasmtimeJavaScriptBackend::new(WasmExecConfig::default());
        let r = backend
            .run("std.exit(4);", Duration::from_secs(10))
            .await
            .expect("supervisor itself succeeds");
        assert_eq!(r.exit_code, Some(4));
        assert!(!r.timed_out);
    }

    #[tokio::test]
    async fn timeout_via_epoch_kills_long_loop() {
        skip_unless_assets!();
        let backend = WasmtimeJavaScriptBackend::new(WasmExecConfig {
            max_timeout: Duration::from_secs(1),
            ..WasmExecConfig::default()
        });
        let r = backend
            .run("while (true) {}", Duration::from_millis(500))
            .await
            .expect("epoch trap reported as timed_out");
        assert!(r.timed_out, "epoch deadline should fire");
        assert_eq!(r.exit_code, None);
    }

    #[tokio::test]
    async fn env_not_visible_in_sandbox() {
        skip_unless_assets!();
        std::env::set_var("XG_JS_LEAK_KEY", "secret");
        let backend = WasmtimeJavaScriptBackend::new(WasmExecConfig::default());
        let r = backend
            .run(
                r"console.log(typeof std !== 'undefined' && std.getenv ? (std.getenv('XG_JS_LEAK_KEY') || 'absent') : 'absent')",
                Duration::from_secs(10),
            )
            .await
            .expect("WASI run");
        std::env::remove_var("XG_JS_LEAK_KEY");
        assert_eq!(r.stdout.trim(), "absent");
    }

    #[tokio::test]
    async fn stdout_cap_truncates_at_64kb() {
        skip_unless_assets!();
        let backend = WasmtimeJavaScriptBackend::new(WasmExecConfig::default());
        let total = OUTPUT_BYTE_CAP + 32 * 1024;
        let snippet = format!("console.log('x'.repeat({total}))");
        let r = backend
            .run(&snippet, Duration::from_secs(10))
            .await
            .expect("WASI run");
        assert!(r.truncated);
        assert!(r.stdout.contains("truncated"));
    }

    #[tokio::test]
    async fn stderr_redaction_applies_to_email() {
        skip_unless_assets!();
        let backend = WasmtimeJavaScriptBackend::new(WasmExecConfig::default());
        let snippet = r#"console.error("contact: alice@example.com")"#;
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
        let backend = WasmtimeJavaScriptBackend::new(WasmExecConfig::default());
        let _warm = backend.run("0", Duration::from_secs(10)).await;
        let start = Instant::now();
        let _ = backend.run("0", Duration::from_secs(10)).await;
        let elapsed = start.elapsed();
        #[cfg(not(debug_assertions))]
        assert!(
            elapsed < Duration::from_millis(200),
            "cold-start cached-path budget blown: {:?}",
            elapsed
        );
        eprintln!("L3 javascript cached cold start: {elapsed:?}");
    }

    #[tokio::test]
    async fn concurrent_calls_do_not_share_state() {
        skip_unless_assets!();
        let backend = std::sync::Arc::new(WasmtimeJavaScriptBackend::new(WasmExecConfig::default()));
        let b1 = backend.clone();
        let b2 = backend.clone();
        let h1 = tokio::spawn(async move {
            b1.run("globalThis.g = 'A'; console.log(globalThis.g)", Duration::from_secs(10))
                .await
        });
        let h2 = tokio::spawn(async move {
            b2.run("globalThis.g = 'B'; console.log(globalThis.g)", Duration::from_secs(10))
                .await
        });
        let r1 = h1.await.unwrap().unwrap();
        let r2 = h2.await.unwrap().unwrap();
        assert!(r1.stdout.contains('A'));
        assert!(r2.stdout.contains('B'));
    }
}
