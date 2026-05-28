//! `xiaoguai-mcp-exec` — sandboxed code-execution MCP server.
//!
//! Spawns short-lived `python3` subprocesses behind a per-call wall-clock
//! deadline, an `ulimit -v` address-space cap, a fresh tempdir CWD, and a
//! scrubbed environment. Standard output is captured to 64 KB; stderr is
//! passed through the workspace PII redactor before return.
//!
//! `HotL` budget gating lives **upstream** in the agent ReAct loop (see
//! `xiaoguai-agent::HotlGate`). This crate is intentionally policy-naive
//! so the same binary can be reused in non-agent contexts (operators
//! testing snippets, eval harnesses).
//!
//! ## Quickstart
//!
//! ```ignore
//! use xiaoguai_mcp_exec::{ExecConfig, run_stdio_server};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     run_stdio_server(ExecConfig::default()).await
//! }
//! ```
//!
//! ## Operator concerns
//!
//! See `docs/runbooks/mcp-exec-sandbox.md` for the threat model, ulimit
//! tuning advice, and the recommended `HotL` policy seed values.

#![forbid(unsafe_code)]

pub mod exec;
pub mod server;
pub mod tools;

pub use exec::{run_python, ExecConfig, ExecError, ExecResult};
pub use server::{run_stdio_server, ExecServer};
