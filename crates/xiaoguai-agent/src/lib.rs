//! Xiaoguai agent loop.
//!
//! v0.1 implements only the trivial single-turn case (no tools, no MCP, no
//! planning). The ReAct + planning + tool dispatch logic arrives in v0.5.

pub mod loop_;

pub use loop_::Agent;
