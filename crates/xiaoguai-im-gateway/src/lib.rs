//! IM gateway library — webhook entry points + provider abstraction.
//!
//! v0.7 ships the common `ImProvider` trait and the Feishu mount. Each
//! provider implementation lives in its own crate; this crate exposes a
//! thin `mount(state, provider)` helper that adds the right routes onto
//! an existing axum router so operators can compose multiple IM channels
//! into one binary.
//!
//! v0.7 wires the webhook → `ReactAgent` path *without* a real reply
//! channel — `Reply::Stub` records what would have been sent so tests can
//! assert on it. Real Feishu `OpenAPI` reply lands in v0.7.1.

#![forbid(unsafe_code)]

pub mod provider;
pub mod router;

pub use provider::{ImEvent, ImProvider, IncomingMessage, OutgoingReply, ProviderError, Webhook};
pub use router::{mount_feishu, run_agent_and_reply, GatewayState};
