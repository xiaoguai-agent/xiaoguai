//! Xiaoguai agent loop.
//!
//! v0.1 shipped only the trivial single-turn case (`Agent::run_once`).
//! v0.5.4 lands the full ReAct loop with parallel MCP tool dispatch,
//! sliding-window history, max-iteration / cancellation guards, and a
//! streaming event channel.

#![forbid(unsafe_code)]

pub mod event;
pub mod history;
pub mod hotl_gate;
pub mod loop_;
pub mod react;
pub mod toolbox;

pub use event::{AgentEvent, StopReason};
pub use hotl_gate::{
    AllowAllGate, DenyAllGate, HotlGate, HotlGateVerdict, ScopeDenyGate, SharedHotlGate,
};
pub use loop_::Agent;
pub use react::{AgentConfig, AgentError, AgentOutcome, ReactAgent};
pub use toolbox::{Toolbox, ToolboxError};
