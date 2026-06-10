//! Xiaoguai agent loop.
//!
//! v0.1 shipped only the trivial single-turn case (`Agent::run_once`).
//! v0.5.4 lands the full ReAct loop with parallel MCP tool dispatch,
//! sliding-window history, max-iteration / cancellation guards, and a
//! streaming event channel.

#![forbid(unsafe_code)]

pub mod consult_gate;
pub mod event;
pub mod history;
pub mod hotl_gate;
pub mod loop_;
pub mod react;
pub mod skill_author_tool;
pub mod toolbox;

pub use consult_gate::{ConsultGate, CONSULT_DENY_REASON};
pub use event::{AgentEvent, HotlResolution, StopReason};
pub use hotl_gate::{
    AllowAllGate, DenyAllGate, HotlGate, HotlGateVerdict, ScopeDenyGate, SharedHotlGate,
};
pub use loop_::Agent;
pub use react::{AgentConfig, AgentError, AgentOutcome, ReactAgent};
pub use skill_author_tool::{
    ProposeSkillArgs, ProposeSkillBackend, ProposeSkillClient, PROPOSE_SKILL_TOOL_NAME,
};
pub use toolbox::{Toolbox, ToolboxError};
