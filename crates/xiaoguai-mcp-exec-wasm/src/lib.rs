//! `xiaoguai-mcp-exec-wasm` — L3 (wasmtime) sandbox backends for both
//! Python (via pyodide) and JavaScript (via QuickJS-WASM).
//!
//! Implements DEC-020 in `xiaoguai-agent-design`. Slots into the
//! `ExecBackend` trait defined in the L1 crates so callers can swap
//! L1 ↔ L3 by config without touching code paths.
//!
//! ## Tier model recap
//!
//! - **L1** (`xiaoguai-mcp-exec`, `xiaoguai-mcp-exec-js`) — subprocess
//!   isolation: fresh tempdir + `ulimit -v` + tokio deadline + scrubbed
//!   env. Fast (~3 ms cold start), works on every host.
//! - **L3** (this crate) — wasmtime capability sandbox: the WASM module
//!   literally cannot call host syscalls. Cold start ~ 50–200 ms after
//!   engine cache warmup. Use when "even a fully adversarial snippet
//!   must not exfiltrate" is the threat model.
//!
//! ## Two binaries, one shared engine
//!
//! The crate ships **two binaries** so operators can deploy only the
//! L3 surface they need:
//! - `xiaoguai-mcp-exec-wasm-py` (Python L3 stdio MCP server)
//! - `xiaoguai-mcp-exec-wasm-js` (JavaScript L3 stdio MCP server)
//!
//! Both share a process-level `wasmtime::Engine` (see [`engine`]); the
//! engine's epoch-tick thread is spawned at first access. If a daemon
//! ever embeds the crate it can call [`engine::shared_engine`] once at
//! boot to warm the cache before first request.
//!
//! ## Asset loading
//!
//! pyodide and QuickJS WASM modules are **NOT** vendored in the crate —
//! they sum to ~15 MB and have their own version cadence. They're
//! loaded at runtime from a path pointed at by environment variables:
//! - `XIAOGUAI_PYODIDE_PATH` → absolute path to `pyodide.wasm`
//! - `XIAOGUAI_QUICKJS_PATH` → absolute path to `quickjs.wasm`
//!
//! When the env var is unset, [`assets::load_pyodide_module`] /
//! [`assets::load_quickjs_module`] return an `AssetMissing` error with
//! an installation hint. Tests gate on this so CI hosts without the
//! assets see clean skips, mirroring the L1 JS `skip_unless_runtime!`
//! idiom.
//!
//! See `scripts/fetch-wasm-assets.sh` (operator-side, documented in the
//! crate README) for the supported versions.
//!
//! ## Defaults
//!
//! - Default `memory_mb` = **256** (pyodide baseline is ~30 MB; the L1
//!   defaults of 512 / 1024 include CPython/Node startup that L3 caches
//!   in the engine).
//! - Default `timeout_secs` = 30 (same as L1).
//! - **No env at all** is exposed to the WASM module. There is no
//!   allow-list today. A future allow-list would be plumbed through a
//!   new field on `WasmExecConfig`.

#![forbid(unsafe_code)]
// doc_markdown is noisy for proper nouns we use heavily — pyodide,
// QuickJS, wasmtime, WASI — and adding backticks to every mention
// would clutter the prose without improving clarity. The workspace
// already allows other noisy pedantic lints case-by-case.
#![allow(clippy::doc_markdown)]

pub mod assets;
pub mod config;
pub mod engine;
pub mod wasmtime_javascript;
pub mod wasmtime_python;

pub use assets::{load_pyodide_module, load_quickjs_module, AssetError};
pub use config::WasmExecConfig;
pub use engine::{shared_engine, ticks_for_secs};
pub use wasmtime_javascript::WasmtimeJavaScriptBackend;
pub use wasmtime_python::WasmtimePythonBackend;
