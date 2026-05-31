//! WASM asset loading (pyodide, QuickJS) from operator-provisioned
//! paths.
//!
//! Per the §6 plan adjustment in
//! `docs/plans/2026-05-30-sprint8-track-a-l3-sandbox.md`, we explicitly
//! do **not** fetch via `build.rs`. Reasons:
//!
//! 1. Sandboxed build environments (CI, agent worktrees) often deny
//!    outbound HTTPS. A build.rs network step would brick the entire
//!    workspace.
//! 2. Reproducibility: build.rs fetches make `cargo clean` re-download.
//! 3. ~40 MB of asset bytes in `target/` per workspace rebuild is
//!    wasteful for a single-crate dependency.
//!
//! Instead: operators run a one-shot
//! `scripts/fetch-wasm-assets.sh` (documented in the crate README) and
//! point the env vars below at the result. Tests gate on the loader
//! succeeding — when assets are absent they print `SKIPPED:` and pass,
//! mirroring the L1 JS `skip_unless_runtime!` idiom.
//!
//! ## Version pins
//!
//! - pyodide: **v0.27.x** (the 0.27 line introduced stable
//!   `__main__.eval` entrypoint we rely on; older versions need a
//!   wrapper module).
//! - QuickJS: **quickjs-emscripten 2025.x** (maintained fork; the
//!   upstream bellard/quickjs has no WASM build).
//!
//! When you bump the version, document the change in the crate README
//! and update the `scripts/fetch-wasm-assets.sh` URL.

use std::path::{Path, PathBuf};

use thiserror::Error;
use wasmtime::{Engine, Module};

/// Env var the Python backend reads when [`WasmExecConfig::module_path`]
/// is `None`. Expected to be an absolute path to a `pyodide.wasm`
/// blob produced by `scripts/fetch-wasm-assets.sh`.
///
/// [`WasmExecConfig::module_path`]: crate::config::WasmExecConfig::module_path
pub const PYODIDE_PATH_ENV: &str = "XIAOGUAI_PYODIDE_PATH";

/// Env var the JavaScript backend reads when
/// [`WasmExecConfig::module_path`] is `None`.
///
/// [`WasmExecConfig::module_path`]: crate::config::WasmExecConfig::module_path
pub const QUICKJS_PATH_ENV: &str = "XIAOGUAI_QUICKJS_PATH";

/// Pinned pyodide upstream version. Surfaced in error messages so
/// operators know which version to fetch.
pub const PYODIDE_VERSION: &str = "0.27.0";

/// Pinned QuickJS-WASM upstream version (quickjs-emscripten fork).
pub const QUICKJS_VERSION: &str = "2025.5.0";

/// Failure paths for asset loading. The `AssetMissing` variant carries
/// the env-var hint so the caller can surface it to the operator.
#[derive(Debug, Error)]
pub enum AssetError {
    /// Neither `config.module_path` nor the asset env var was set.
    #[error(
        "{language} WASM asset not provisioned. Set ${env_var} to the absolute path of \
         {asset_filename} (pinned version {version}). See scripts/fetch-wasm-assets.sh."
    )]
    AssetMissing {
        language: &'static str,
        env_var: &'static str,
        asset_filename: &'static str,
        version: &'static str,
    },
    /// The path exists but reading or compiling the WASM failed.
    #[error("read/compile {0}: {1}")]
    LoadFailed(PathBuf, String),
}

/// Resolve the pyodide WASM module path: prefer the explicit
/// `override_path`, fall back to `PYODIDE_PATH_ENV`, fail with
/// [`AssetError::AssetMissing`] if neither is set.
pub fn resolve_pyodide_path(override_path: Option<&Path>) -> Result<PathBuf, AssetError> {
    if let Some(p) = override_path {
        return Ok(p.to_path_buf());
    }
    std::env::var_os(PYODIDE_PATH_ENV)
        .map(PathBuf::from)
        .ok_or(AssetError::AssetMissing {
            language: "python",
            env_var: PYODIDE_PATH_ENV,
            asset_filename: "pyodide.wasm",
            version: PYODIDE_VERSION,
        })
}

/// Resolve the QuickJS WASM module path. Mirror of
/// [`resolve_pyodide_path`].
pub fn resolve_quickjs_path(override_path: Option<&Path>) -> Result<PathBuf, AssetError> {
    if let Some(p) = override_path {
        return Ok(p.to_path_buf());
    }
    std::env::var_os(QUICKJS_PATH_ENV)
        .map(PathBuf::from)
        .ok_or(AssetError::AssetMissing {
            language: "javascript",
            env_var: QUICKJS_PATH_ENV,
            asset_filename: "quickjs.wasm",
            version: QUICKJS_VERSION,
        })
}

/// Read the pyodide WASM blob and compile it against `engine`.
/// Module compilation is cached by wasmtime internally; callers
/// should `OnceLock` the returned `Module` anyway to avoid the
/// fs read on every call.
pub fn load_pyodide_module(
    engine: &Engine,
    override_path: Option<&Path>,
) -> Result<Module, AssetError> {
    let path = resolve_pyodide_path(override_path)?;
    Module::from_file(engine, &path).map_err(|e| AssetError::LoadFailed(path, e.to_string()))
}

/// Read the QuickJS WASM blob and compile it. Mirror of
/// [`load_pyodide_module`].
pub fn load_quickjs_module(
    engine: &Engine,
    override_path: Option<&Path>,
) -> Result<Module, AssetError> {
    let path = resolve_quickjs_path(override_path)?;
    Module::from_file(engine, &path).map_err(|e| AssetError::LoadFailed(path, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guard against env var leakage between tests. We snapshot and
    /// restore.
    struct EnvGuard {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }
    impl EnvGuard {
        fn unset(key: &'static str) -> Self {
            let prev = std::env::var_os(key);
            std::env::remove_var(key);
            Self { key, prev }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn load_pyodide_returns_asset_missing_when_unset() {
        let _g = EnvGuard::unset(PYODIDE_PATH_ENV);
        let engine = crate::engine::shared_engine();
        let err = load_pyodide_module(engine, None).expect_err("asset must be missing");
        match err {
            AssetError::AssetMissing { env_var, .. } => {
                assert_eq!(env_var, PYODIDE_PATH_ENV);
            }
            other => panic!("expected AssetMissing, got {other:?}"),
        }
    }

    #[test]
    fn load_quickjs_returns_asset_missing_when_unset() {
        let _g = EnvGuard::unset(QUICKJS_PATH_ENV);
        let engine = crate::engine::shared_engine();
        let err = load_quickjs_module(engine, None).expect_err("asset must be missing");
        match err {
            AssetError::AssetMissing { env_var, .. } => {
                assert_eq!(env_var, QUICKJS_PATH_ENV);
            }
            other => panic!("expected AssetMissing, got {other:?}"),
        }
    }

    #[test]
    fn asset_missing_error_includes_install_hint() {
        let err = AssetError::AssetMissing {
            language: "python",
            env_var: PYODIDE_PATH_ENV,
            asset_filename: "pyodide.wasm",
            version: PYODIDE_VERSION,
        };
        let msg = err.to_string();
        // Operators searching the runbook by version pin or fetch
        // script name should see them in the error.
        assert!(
            msg.contains(PYODIDE_PATH_ENV),
            "missing env var hint: {msg}"
        );
        assert!(
            msg.contains("fetch-wasm-assets.sh"),
            "missing script hint: {msg}"
        );
        assert!(msg.contains(PYODIDE_VERSION), "missing version hint: {msg}");
    }
}
