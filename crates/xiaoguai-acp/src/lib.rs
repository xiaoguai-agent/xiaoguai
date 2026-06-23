//! Agent Client Protocol (ACP) stdio adapter — DEC-038 / `LLD-ACP-001`.
//!
//! Exposes the existing `xiaoguai-agent` ReAct loop to code editors that speak
//! the [Agent Client Protocol](https://agentclientprotocol.com). The IDE is a
//! *transport/surface*: a prompt arriving over ACP runs the same loop as one
//! from chat-ui, the CLI, or an IM adapter — so any **tool call** it makes is
//! gated by the same `HotL` gate and signed into the same audit chain. This
//! crate mirrors the IM-gateway adapter pattern — a thin protocol shell over
//! `xiaoguai-runtime`.
//!
//! Note: in P2 the ACP `RuntimeContext` is built with an **empty toolbox**
//! (coding-tool registration into the ACP path is the deferred follow-up,
//! `LLD-ACP-001` §6), so today an ACP turn is chat-only — there are no tool
//! calls yet to gate or audit. The governance machinery is inherited from the
//! loop and activates as soon as a toolbox is wired in.
//!
//! ## What lives here
//!
//! * [`jsonrpc`] — a minimal JSON-RPC 2.0 envelope + the standard error codes.
//! * [`transport`] — newline-delimited framing over any `AsyncRead`/`AsyncWrite`
//!   (ACP over stdio is one JSON object per line, `\n`-terminated; **not** LSP
//!   `Content-Length` framing). Generic so the whole protocol is exercised over
//!   an in-memory `duplex` pipe in tests.
//! * [`delegate`] — [`AcpDelegate`], the decoupling seam: the server knows
//!   nothing about the agent; a delegate drives one prompt turn and emits
//!   updates through an [`UpdateSink`].
//! * [`mapping`] — pure `AgentEvent` → ACP `SessionUpdate` translation.
//! * [`server`] — the dispatch loop: routes `initialize` / `session/new` /
//!   `session/prompt` / `session/cancel` to the delegate.
//! * [`runtime_delegate`] — an [`AcpDelegate`] backed by a
//!   [`xiaoguai_runtime::RuntimeContext`] (the CLI path).
//!
//! ## Wire contracts
//!
//! Message types are taken verbatim from the upstream
//! [`agent_client_protocol_schema`] crate (re-exported here as [`acp`]). This is
//! the standing rule against guessing wire shapes: we own the transport loop,
//! not the contracts.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

/// The ACP wire schema, re-exported so call sites use one canonical path.
///
/// Upstream v0.14 moved the per-method message types under a `v1` module
/// (`v2` is feature-gated). We flatten `v1` here — together with the
/// crate-root `ProtocolVersion` newtype, which lives outside the version
/// modules — so call sites keep the flat `acp::Foo` path and the wire
/// contract stays pinned to one upstream version in this one place.
pub mod acp {
    pub use agent_client_protocol_schema::v1::*;
    pub use agent_client_protocol_schema::ProtocolVersion;
}

pub mod delegate;
pub mod jsonrpc;
pub mod mapping;
pub mod runtime_delegate;
pub mod server;
pub mod transport;

pub use delegate::{AcpDelegate, UpdateSink};
pub use runtime_delegate::RuntimeDelegate;
pub use server::serve;

/// Protocol version this adapter implements. ACP negotiates the *minimum* of
/// the client's and the agent's latest, so advertising v1 is forward-safe.
pub const PROTOCOL_VERSION: u16 = 1;

/// Method names handled by the agent side of the connection. Kept as local
/// constants because the schema crate marks its equivalents `pub(crate)`; these
/// are the stable wire strings from the ACP spec, not guesses.
pub mod methods {
    /// Negotiate protocol version + capabilities (request).
    pub const INITIALIZE: &str = "initialize";
    /// Create a new conversation session (request).
    pub const SESSION_NEW: &str = "session/new";
    /// Send a user prompt; drives one turn (request).
    pub const SESSION_PROMPT: &str = "session/prompt";
    /// Cancel the in-flight turn for a session (notification).
    pub const SESSION_CANCEL: &str = "session/cancel";
}
