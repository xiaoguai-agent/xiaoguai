//! `xiaoguai-mcp-exec-js` — sandboxed JavaScript code-execution MCP server.
//!
//! Spawns short-lived `deno` (default) or `node` subprocesses behind a
//! per-call wall-clock deadline, an `ulimit -v` address-space cap, a
//! fresh tempdir CWD, and a scrubbed environment. Standard output is
//! captured to 64 KB; stderr is passed through the workspace PII
//! redactor before return.
//!
//! ## Separate trust boundary from `xiaoguai-mcp-exec`
//!
//! This crate is a **sibling** of `xiaoguai-mcp-exec` (the Python
//! sandbox), not a fork. We keep separate binaries, separate `HotL`
//! scopes, and separate runbooks so a sandbox escape in one runtime
//! cannot chain into the other.
//!
//! `HotL` budget gating lives **upstream** in the agent ReAct loop (see
//! `xiaoguai-agent::HotlGate`). This crate is intentionally policy-naive
//! so the same binary can be reused in non-agent contexts (operators
//! testing snippets, eval harnesses).
//!
//! ## Quickstart
//!
//! ```ignore
//! use xiaoguai_mcp_exec_js::{ExecConfig, run_stdio_server};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     run_stdio_server(ExecConfig::default()).await
//! }
//! ```
//!
//! ## Operator concerns
//!
//! See `docs/runbooks/mcp-exec-js-sandbox.md` for the threat model,
//! runtime install (Deno vs Node), ulimit tuning advice, and the
//! recommended `HotL` policy seed values.

#![forbid(unsafe_code)]

pub mod exec;
pub mod runtime;
pub mod server;
pub mod tools;

pub use exec::{run_javascript, ExecConfig, ExecError, ExecResult, Runtime};
pub use runtime::{CapabilitySummary, ExecBackend, ProcessL1JavaScript};
pub use server::{run_stdio_server, ExecServer};
