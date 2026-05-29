//! Shared `WasmExecConfig` for both L3 backends.
//!
//! Structurally similar to the L1 `ExecConfig`s but with WASM-specific
//! tunables and lower defaults (pyodide baseline is ~30 MB, not 512 MB
//! like CPython startup).

use std::path::PathBuf;
use std::time::Duration;

/// Configuration for a single L3 execution call. Shared by both the
/// Python and JavaScript backends because the knobs are identical
/// (WASM doesn't care which interpreter it's hosting).
#[derive(Clone, Debug)]
pub struct WasmExecConfig {
    /// Hard wall-clock cap. Per-call timeouts above this are clamped.
    /// Enforced via wasmtime epoch interruption (1 tick = 10 ms).
    pub max_timeout: Duration,

    /// Memory ceiling in megabytes. Enforced via
    /// `wasmtime::Store::limiter` — the WASM module sees an OOM trap
    /// when it tries to grow its linear memory past this.
    ///
    /// Default **256 MB**: pyodide's baseline image is ~30 MB and most
    /// snippets need < 100 MB working memory. L1's 512 MB / 1024 MB
    /// defaults reflect CPython/Node startup overhead that L3 caches
    /// in the engine.
    pub memory_mb: u64,

    /// When true, stderr passes through the PII redactor before return.
    /// Same posture as L1: scrub by default, opt-out for operators
    /// debugging their own snippets.
    pub redact_stderr: bool,

    /// Optional override for the WASM module path. When `None`, the
    /// backend reads the asset-specific env var
    /// (`XIAOGUAI_PYODIDE_PATH` / `XIAOGUAI_QUICKJS_PATH`) via the
    /// `assets` module.
    pub module_path: Option<PathBuf>,
}

impl Default for WasmExecConfig {
    fn default() -> Self {
        Self {
            max_timeout: Duration::from_secs(30),
            memory_mb: 256,
            redact_stderr: true,
            module_path: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_memory_is_256_mb() {
        assert_eq!(WasmExecConfig::default().memory_mb, 256);
    }

    #[test]
    fn default_timeout_is_30_secs() {
        assert_eq!(WasmExecConfig::default().max_timeout.as_secs(), 30);
    }

    #[test]
    fn default_redacts_stderr() {
        assert!(WasmExecConfig::default().redact_stderr);
    }

    #[test]
    fn default_module_path_is_unset() {
        assert!(WasmExecConfig::default().module_path.is_none());
    }
}
