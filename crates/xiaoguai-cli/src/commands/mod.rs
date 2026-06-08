//! Subcommand implementations.
//!
//! Each module exposes pure functions taking the dependencies they need
//! (typically a repository trait object) so they are unit-testable without
//! involving clap or `assert_cmd`.

pub mod anomaly;
pub mod audit_bundle;
pub mod audit_export;
pub mod backup;
pub mod chat;
pub mod code;
pub mod completions;
pub mod eval;
pub mod hotl;
pub mod init;
pub mod r#loop;
pub mod manpages;
pub mod mcp;
pub mod outcomes;
pub mod provider;
pub mod remote;
pub mod schedule;
pub mod self_update;
pub mod skills;
pub mod stats;
pub mod tasks;
pub mod watch;
