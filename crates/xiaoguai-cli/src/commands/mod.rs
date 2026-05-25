//! Subcommand implementations.
//!
//! Each module exposes pure functions taking the dependencies they need
//! (typically a repository trait object) so they are unit-testable without
//! involving clap or `assert_cmd`.

pub mod audit;
pub mod backup;
pub mod chat;
pub mod completions;
pub mod eval;
pub mod manpages;
pub mod mcp;
pub mod provider;
pub mod remote;
pub mod self_update;
pub mod tasks;
