//! `xiaoguai-mcp-exec` — sandboxed code-execution MCP server.
//!
//! Spawns short-lived `python3` subprocesses behind a per-call wall-clock
//! deadline, in-process rlimits applied via `pre_exec` + `setrlimit(2)`
//! (SEC-10/#289: best-effort `RLIMIT_AS` address-space cap, enforced
//! `RLIMIT_NPROC` process-count and `RLIMIT_FSIZE` file-size caps), a fresh
//! tempdir CWD, and a scrubbed environment. Standard output is captured to
//! 64 KB; stderr is passed through the workspace PII redactor before return.
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
//! See `docs/runbooks/mcp-exec-sandbox.md` for the threat model, rlimit
//! tuning advice, and the recommended `HotL` policy seed values.

// #289: `deny` rather than the workspace-wide `forbid` — the sandbox needs
// exactly two scoped `#[allow(unsafe_code)]` sites in `exec.rs` for the
// `pre_exec` + `setrlimit(2)` resource caps (shell `ulimit` was unreliable
// under dash). Any new unsafe must carry its own justified allow.
#![deny(unsafe_code)]

pub mod exec;
pub mod runtime;
pub mod server;
pub mod tools;

pub use exec::{run_python, ExecConfig, ExecError, ExecResult};
pub use runtime::{CapabilitySummary, ExecBackend, ProcessL1Python, ACK_UNISOLATED_ENV};
pub use server::{run_stdio_server, ExecServer};
