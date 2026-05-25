//! Built-in MCP server adapters.
//!
//! Each sub-module is an in-process REST/API adapter that the pack runner
//! wires as an MCP tool server without spawning a child process.
//!
//! Current adapters:
//!   - `github_pr` — GitHub REST API: `get_pr_diff`, `post_pr_review`, `post_comment`

pub mod github_pr;
