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

/// Optional integrity pin for the pyodide blob: a lowercase hex SHA-256.
/// When set, the loaded bytes must match or loading fails. Unset = no check
/// (the default — the canonical digest depends on the operator's fetch).
pub const PYODIDE_SHA256_ENV: &str = "XIAOGUAI_PYODIDE_SHA256";

/// Optional integrity pin for the QuickJS blob. Mirror of
/// [`PYODIDE_SHA256_ENV`].
pub const QUICKJS_SHA256_ENV: &str = "XIAOGUAI_QUICKJS_SHA256";

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
    /// An integrity pin (`*_SHA256` env var) was set but the asset's digest
    /// did not match.
    #[error(
        "{language} WASM asset at {path} failed integrity check: expected sha256 {expected}, \
         got {actual}. Update ${env_var} or re-fetch the pinned asset."
    )]
    IntegrityMismatch {
        language: &'static str,
        path: PathBuf,
        env_var: &'static str,
        expected: String,
        actual: String,
    },
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

/// Read a WASM blob, optionally verify its SHA-256 against the pin in
/// `sha_env` (when set), then compile it against `engine`.
fn load_module_pinned(
    engine: &Engine,
    path: PathBuf,
    language: &'static str,
    sha_env: &'static str,
) -> Result<Module, AssetError> {
    let bytes =
        std::fs::read(&path).map_err(|e| AssetError::LoadFailed(path.clone(), e.to_string()))?;

    if let Some(raw) = std::env::var_os(sha_env) {
        if raw.to_string_lossy().trim().is_empty() {
            // Set-but-empty pin (e.g. CI templating left it blank): the check
            // is OFF. Say so — the operator likely believes it is active.
            tracing::warn!(
                env_var = sha_env,
                "integrity pin set but empty — check disabled"
            );
        }
        if let Err((expected, actual)) = check_pin(&bytes, &raw.to_string_lossy()) {
            return Err(AssetError::IntegrityMismatch {
                language,
                path,
                env_var: sha_env,
                expected,
                actual,
            });
        }
    }

    Module::new(engine, &bytes).map_err(|e| AssetError::LoadFailed(path, e.to_string()))
}

/// Verify `bytes` against an expected lowercase-hex SHA-256 pin. An empty /
/// whitespace pin means "no check" (`Ok`). On mismatch returns
/// `(normalised_expected, actual)`.
fn check_pin(bytes: &[u8], expected_raw: &str) -> Result<(), (String, String)> {
    use sha2::{Digest, Sha256};
    let expected = expected_raw.trim().to_ascii_lowercase();
    if expected.is_empty() {
        return Ok(());
    }
    let actual = hex::encode(Sha256::digest(bytes));
    if actual == expected {
        Ok(())
    } else {
        Err((expected, actual))
    }
}

/// Read the pyodide WASM blob and compile it against `engine`.
/// Module compilation is cached by wasmtime internally; callers
/// should `OnceLock` the returned `Module` anyway to avoid the
/// fs read on every call. When `PYODIDE_SHA256_ENV` is set, the blob's
/// digest is verified before compilation.
pub fn load_pyodide_module(
    engine: &Engine,
    override_path: Option<&Path>,
) -> Result<Module, AssetError> {
    let path = resolve_pyodide_path(override_path)?;
    load_module_pinned(engine, path, "python", PYODIDE_SHA256_ENV)
}

/// Read the QuickJS WASM blob and compile it. Mirror of
/// [`load_pyodide_module`].
pub fn load_quickjs_module(
    engine: &Engine,
    override_path: Option<&Path>,
) -> Result<Module, AssetError> {
    let path = resolve_quickjs_path(override_path)?;
    load_module_pinned(engine, path, "javascript", QUICKJS_SHA256_ENV)
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

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(bytes))
    }

    #[test]
    fn check_pin_empty_skips() {
        assert!(check_pin(b"anything", "").is_ok());
        assert!(check_pin(b"anything", "   ").is_ok());
    }

    #[test]
    fn check_pin_accepts_matching_digest_case_insensitive() {
        let digest = sha256_hex(b"hello world").to_uppercase();
        assert!(check_pin(b"hello world", &format!("  {digest}  ")).is_ok());
    }

    #[test]
    fn check_pin_rejects_mismatch() {
        let wrong = "0".repeat(64);
        let err = check_pin(b"hello world", &wrong).expect_err("must mismatch");
        assert_eq!(err.0, wrong);
        assert_eq!(err.1, sha256_hex(b"hello world"));
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
