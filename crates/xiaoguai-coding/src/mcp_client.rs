//! In-process MCP client exposing the governed coding tools to the ReAct loop.
//!
//! The agent `Toolbox` (`xiaoguai-agent`) registers every tool as an
//! [`McpClient`]; the loop then surfaces each tool to the model, `HotL`-gates it
//! on `tool_call.<name>`, and dispatches via `call_tool`. Wrapping
//! [`GovernedTools`] in an `McpClient` is therefore all it takes to make the
//! coding tools *first-class agent tools* — the loop provides the `HotL` gate +
//! generic audit, and `GovernedTools` adds the checkpoint + the coding-specific
//! `code.*` / `git.*` audit rows (carrying the checkpoint id for rollback).
//!
//! The wrapper is generic over the gate + recorder so the coding crate stays
//! free of the concrete `HotL`/audit types (those live in `xiaoguai-core`). For
//! in-loop use core passes an allow-all `CodingGate` — the loop already enforced
//! the `HotL` decision, so re-gating here would be double-gating.
//!
//! Only the **governed** surface is exposed. `git_branch` is intentionally
//! omitted: it has no governed wrapper (it would bypass the
//! gate/checkpoint/audit sequence), so it must not be reachable by the model.

use std::path::Path;

use async_trait::async_trait;
use serde_json::{json, Value};
use xiaoguai_mcp::{McpClient, McpResult, MutationHint, ServerInfo, ToolDescriptor, ToolResult};

use crate::{CheckpointId, CodingGate, FileEdit, GovernedTools, StepRecorder};

/// An [`McpClient`] over a [`GovernedTools`] instance bound to one workspace.
pub struct CodingMcpClient<G, R> {
    tools: GovernedTools<G, R>,
    /// Whether the egress tools (`git_push`/`open_pr`) are advertised by
    /// `list_tools`. Dispatch still handles them if called, but the loop only
    /// surfaces what is registered in the toolbox.
    include_egress: bool,
}

impl<G, R> CodingMcpClient<G, R> {
    /// Wrap an already-built governed-tools facade. `include_egress` controls
    /// whether `list_tools` advertises the network/past-undo tools.
    pub fn new(tools: GovernedTools<G, R>, include_egress: bool) -> Self {
        Self {
            tools,
            include_egress,
        }
    }
}

/// The tool catalogue this client exposes. Free-standing so callers can
/// introspect names/schemas without constructing a workspace.
///
/// `include_egress` gates the two **network/past-undo** tools (`git_push`,
/// `open_pr`): they leave the local machine and cannot be rolled back, so they
/// are opt-in — a default deployment exposes only the workspace-contained,
/// checkpoint-reversible tools.
#[must_use]
pub fn coding_tool_descriptors(include_egress: bool) -> Vec<ToolDescriptor> {
    let mut tools = vec![
        descriptor(
            "read_file",
            "[READ] Read a UTF-8 text file relative to the workspace root. \
             Use before editing so you base changes on current contents. \
             Args: path (string, required). Errors if the file is absent.",
            json!({
                "type": "object",
                "properties": { "path": { "type": "string", "description": "Path relative to the workspace root." } },
                "required": ["path"]
            }),
            MutationHint::Read,
        ),
        descriptor(
            "list_dir",
            "[READ] List a directory's entries (one per line, dirs suffixed `/`), \
             sorted. Args: path (string, optional; defaults to the workspace root).",
            json!({
                "type": "object",
                "properties": { "path": { "type": "string", "description": "Directory path relative to the workspace root; defaults to '.'." } }
            }),
            MutationHint::Read,
        ),
        descriptor(
            "grep",
            "[READ] Search the workspace for a substring/pattern and return \
             matching `path:line: text` lines. Args: pattern (string, required). \
             Prefer this over reading whole files to locate code.",
            json!({
                "type": "object",
                "properties": { "pattern": { "type": "string", "description": "Text/pattern to search for." } },
                "required": ["pattern"]
            }),
            MutationHint::Read,
        ),
        descriptor(
            "git_status",
            "[READ] Porcelain git status of the workspace (empty ⇒ clean). \
             No args. Use to see what you've changed before committing.",
            json!({ "type": "object", "properties": {} }),
            MutationHint::Read,
        ),
        descriptor(
            "edit_file",
            "[WRITE] Edit a file, taking a checkpoint first (reversible via \
             rollback) and signing a `code.edit` audit row. Either pass `content` \
             to replace the whole file (creating it + parent dirs if absent), OR \
             `find`/`replace` for a literal search-replace. Args: path (required); \
             then content, OR find (+ optional replace, all). A missing `find` is \
             an error — no silent no-op.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path relative to the workspace root." },
                    "content": { "type": "string", "description": "Whole-file replacement contents." },
                    "find": { "type": "string", "description": "Literal text to find (search-replace mode)." },
                    "replace": { "type": "string", "description": "Replacement text (defaults to empty)." },
                    "all": { "type": "boolean", "description": "Replace all occurrences (default: first only)." }
                },
                "required": ["path"]
            }),
            MutationHint::Write,
        ),
        descriptor(
            "git_commit",
            "[WRITE] Stage all changes and commit, taking a checkpoint first and \
             signing a `git.commit` audit row. Args: message (string, required). \
             Returns the commit SHA + checkpoint id.",
            json!({
                "type": "object",
                "properties": { "message": { "type": "string", "description": "Commit message." } },
                "required": ["message"]
            }),
            MutationHint::Write,
        ),
        descriptor(
            "rollback",
            "[WRITE] Restore the working tree to a checkpoint (the id printed by \
             a prior edit/commit), signing a `code.rollback` audit row. Args: \
             checkpoint (string, required). Does NOT undo a completed push/PR.",
            json!({
                "type": "object",
                "properties": { "checkpoint": { "type": "string", "description": "Checkpoint id (a commit SHA from a prior edit/commit)." } },
                "required": ["checkpoint"]
            }),
            MutationHint::Write,
        ),
    ];
    if include_egress {
        tools.push(descriptor(
            "git_push",
            "[WRITE] Push a branch to a remote (egress; signs `git.push`). Args: \
             branch (string, required); remote (string, optional, default \
             'origin'). This leaves the local machine — gated accordingly.",
            json!({
                "type": "object",
                "properties": {
                    "branch": { "type": "string", "description": "Branch to push." },
                    "remote": { "type": "string", "description": "Remote name (default 'origin')." }
                },
                "required": ["branch"]
            }),
            MutationHint::Write,
        ));
        tools.push(descriptor(
            "open_pr",
            "[WRITE] Open a pull request via `gh` (egress; signs `pr.open`). Args: \
             title (required); body (optional); base (optional, default 'main'). \
             Returns the PR URL.",
            json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "PR title." },
                    "body": { "type": "string", "description": "PR body (default empty)." },
                    "base": { "type": "string", "description": "Base branch (default 'main')." }
                },
                "required": ["title"]
            }),
            MutationHint::Write,
        ));
    }
    tools
}

/// Build one coding-tool descriptor. `mutation_hint` is set explicitly from
/// the tool's `[READ]`/`[WRITE]` description tag (plan §5.2); the textual
/// tags are kept for the model's benefit.
fn descriptor(
    name: &str,
    description: &str,
    input_schema: Value,
    mutation_hint: MutationHint,
) -> ToolDescriptor {
    ToolDescriptor {
        name: name.to_string(),
        description: Some(description.to_string()),
        input_schema,
        mutation_hint,
    }
}

#[async_trait]
impl<G, R> McpClient for CodingMcpClient<G, R>
where
    G: CodingGate + 'static,
    R: StepRecorder + 'static,
{
    async fn initialize(&self) -> McpResult<ServerInfo> {
        Ok(ServerInfo {
            name: "xiaoguai-coding".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        })
    }

    async fn list_tools(&self) -> McpResult<Vec<ToolDescriptor>> {
        Ok(coding_tool_descriptors(self.include_egress))
    }

    async fn call_tool(&self, name: &str, args: Value) -> McpResult<ToolResult> {
        // Tool-level failures are returned as `is_error` results (not transport
        // errors) so the model sees the teaching message and can recover.
        Ok(match self.dispatch(name, &args).await {
            Ok(text) => text_result(text, false),
            Err(message) => text_result(message, true),
        })
    }

    async fn shutdown(&self) -> McpResult<()> {
        Ok(())
    }
}

impl<G, R> CodingMcpClient<G, R>
where
    G: CodingGate,
    R: StepRecorder,
{
    async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "read_file" => self
                .tools
                .workspace()
                .read_file(Path::new(&str_arg(args, "path")?))
                .await
                .map_err(|e| e.to_string()),
            "list_dir" => {
                let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
                let entries = self
                    .tools
                    .workspace()
                    .list_dir(Path::new(path))
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(join_or(entries, "(empty)"))
            }
            "grep" => {
                let hits = self
                    .tools
                    .workspace()
                    .grep(&str_arg(args, "pattern")?)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(join_or(hits, "(no matches)"))
            }
            "git_status" => {
                let out = self
                    .tools
                    .workspace()
                    .git_status()
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(if out.trim().is_empty() {
                    "(clean)".to_string()
                } else {
                    out
                })
            }
            "edit_file" => {
                let path = str_arg(args, "path")?;
                let edit = parse_edit(args)?;
                let out = self
                    .tools
                    .edit_file(Path::new(&path), &edit)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(format!(
                    "edited {} (+{} repl, {} bytes) — checkpoint {}",
                    out.result.path.display(),
                    out.result.replacements,
                    out.result.bytes_after,
                    out.checkpoint
                ))
            }
            "git_commit" => {
                let out = self
                    .tools
                    .git_commit(&str_arg(args, "message")?)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(format!(
                    "committed {} — checkpoint {}",
                    out.result, out.checkpoint
                ))
            }
            "rollback" => {
                let cp = str_arg(args, "checkpoint")?;
                self.tools
                    .rollback(&CheckpointId::from_sha(cp.clone()))
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(format!("rolled back to {cp}"))
            }
            "git_push" => {
                let remote = args
                    .get("remote")
                    .and_then(Value::as_str)
                    .unwrap_or("origin")
                    .to_string();
                let branch = str_arg(args, "branch")?;
                let out = self
                    .tools
                    .git_push(&remote, &branch)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(if out.trim().is_empty() {
                    format!("pushed {remote} {branch}")
                } else {
                    format!("pushed {remote} {branch}\n{out}")
                })
            }
            "open_pr" => {
                let title = str_arg(args, "title")?;
                let body = args.get("body").and_then(Value::as_str).unwrap_or("");
                let base = args.get("base").and_then(Value::as_str).unwrap_or("main");
                let url = self
                    .tools
                    .open_pr(&title, body, base)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(format!("opened PR: {url}"))
            }
            other => Err(format!(
                "unknown coding tool `{other}`; available: read_file, list_dir, \
                 grep, git_status, edit_file, git_commit, rollback, git_push, open_pr"
            )),
        }
    }
}

/// Extract a required string argument, with a teaching error if absent.
fn str_arg(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| format!("missing required string argument `{key}`"))
}

/// Build a `FileEdit` from `edit_file` args: `content` ⇒ whole-file write;
/// `find`(+`replace`,`all`) ⇒ search-replace. The two modes are mutually
/// exclusive (passing both is an error, not a silent preference), and an empty
/// `find` is rejected (it would otherwise splice `replace` between every char).
fn parse_edit(args: &Value) -> Result<FileEdit, String> {
    let content = args.get("content").and_then(Value::as_str);
    let find = args.get("find").and_then(Value::as_str);
    match (content, find) {
        (Some(_), Some(_)) => Err("pass either `content` or `find`, not both".to_string()),
        (Some(content), None) => Ok(FileEdit::Write(content.to_string())),
        (None, Some(find)) => {
            if find.is_empty() {
                return Err("`find` must be non-empty".to_string());
            }
            let replace = args
                .get("replace")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
            Ok(FileEdit::Replace {
                find: find.to_string(),
                replace,
                all,
            })
        }
        (None, None) => Err(
            "edit_file needs either `content` (whole-file write) or `find` (search-replace)"
                .to_string(),
        ),
    }
}

fn join_or(lines: Vec<String>, empty: &str) -> String {
    if lines.is_empty() {
        empty.to_string()
    } else {
        lines.join("\n")
    }
}

fn text_result(text: String, is_error: bool) -> ToolResult {
    ToolResult {
        text,
        blocks: vec![],
        is_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governed::{CodingStep, GateDecision};
    use crate::Workspace;

    struct AllowGate;
    #[async_trait]
    impl CodingGate for AllowGate {
        async fn decide(&self, _scope: &str) -> GateDecision {
            GateDecision::Allow
        }
    }

    struct NoopRecorder;
    #[async_trait]
    impl StepRecorder for NoopRecorder {
        async fn record(&self, _step: CodingStep) {}
    }

    async fn client() -> (CodingMcpClient<AllowGate, NoopRecorder>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open_or_create(dir.path()).await.unwrap();
        let tools = GovernedTools::new(ws, AllowGate, NoopRecorder);
        (CodingMcpClient::new(tools, true), dir)
    }

    #[tokio::test]
    async fn edit_then_read_round_trips_and_checkpoints() {
        let (c, _dir) = client().await;
        let edit = c
            .call_tool("edit_file", json!({ "path": "a.txt", "content": "hello" }))
            .await
            .unwrap();
        assert!(!edit.is_error, "edit should succeed: {}", edit.text);
        assert!(edit.text.contains("checkpoint"), "got: {}", edit.text);

        let read = c
            .call_tool("read_file", json!({ "path": "a.txt" }))
            .await
            .unwrap();
        assert!(!read.is_error);
        assert_eq!(read.text, "hello");
    }

    #[tokio::test]
    async fn search_replace_edit() {
        let (c, _dir) = client().await;
        c.call_tool(
            "edit_file",
            json!({ "path": "a.txt", "content": "foo bar foo" }),
        )
        .await
        .unwrap();
        let edit = c
            .call_tool(
                "edit_file",
                json!({ "path": "a.txt", "find": "foo", "replace": "baz", "all": true }),
            )
            .await
            .unwrap();
        assert!(!edit.is_error, "{}", edit.text);
        let read = c
            .call_tool("read_file", json!({ "path": "a.txt" }))
            .await
            .unwrap();
        assert_eq!(read.text, "baz bar baz");
    }

    #[tokio::test]
    async fn missing_arg_is_a_tool_error_not_a_panic() {
        let (c, _dir) = client().await;
        let res = c
            .call_tool("edit_file", json!({ "content": "x" }))
            .await
            .unwrap();
        assert!(res.is_error);
        assert!(res.text.contains("path"), "got: {}", res.text);
    }

    #[tokio::test]
    async fn unknown_tool_is_a_tool_error() {
        let (c, _dir) = client().await;
        let res = c.call_tool("teleport", json!({})).await.unwrap();
        assert!(res.is_error);
        assert!(res.text.contains("unknown coding tool"));
    }

    #[tokio::test]
    async fn edit_with_both_content_and_find_is_rejected() {
        let (c, _dir) = client().await;
        let res = c
            .call_tool(
                "edit_file",
                json!({ "path": "a.txt", "content": "x", "find": "y" }),
            )
            .await
            .unwrap();
        assert!(res.is_error);
        assert!(res.text.contains("not both"), "got: {}", res.text);
    }

    #[tokio::test]
    async fn edit_with_empty_find_is_rejected() {
        let (c, _dir) = client().await;
        c.call_tool("edit_file", json!({ "path": "a.txt", "content": "abc" }))
            .await
            .unwrap();
        let res = c
            .call_tool("edit_file", json!({ "path": "a.txt", "find": "" }))
            .await
            .unwrap();
        assert!(res.is_error);
        assert!(res.text.contains("non-empty"), "got: {}", res.text);
    }

    #[tokio::test]
    async fn path_traversal_is_rejected() {
        let (c, _dir) = client().await;
        for bad in ["../escape.txt", "/etc/passwd", "a/../../b.txt"] {
            let res = c
                .call_tool("edit_file", json!({ "path": bad, "content": "x" }))
                .await
                .unwrap();
            assert!(res.is_error, "{bad} should be rejected");
            assert!(res.text.contains("unsafe path"), "got: {}", res.text);
        }
    }

    #[test]
    fn mutation_hints_match_read_write_description_tags() {
        // T5 plan §5.2: the structured hint must agree with the textual
        // [READ]/[WRITE] tag every descriptor already carries.
        for d in coding_tool_descriptors(true) {
            let desc = d.description.expect("coding tools are documented");
            let expected = if desc.starts_with("[READ]") {
                MutationHint::Read
            } else {
                assert!(
                    desc.starts_with("[WRITE]"),
                    "{} has no [READ]/[WRITE] tag",
                    d.name
                );
                MutationHint::Write
            };
            assert_eq!(d.mutation_hint, expected, "hint/tag mismatch on {}", d.name);
        }
    }

    #[test]
    fn read_tools_are_exactly_the_observation_set() {
        let mut reads: Vec<String> = coding_tool_descriptors(true)
            .into_iter()
            .filter(|d| d.mutation_hint == MutationHint::Read)
            .map(|d| d.name)
            .collect();
        reads.sort();
        assert_eq!(reads, ["git_status", "grep", "list_dir", "read_file"]);
    }

    #[test]
    fn descriptors_gate_egress() {
        let no_egress: Vec<_> = coding_tool_descriptors(false)
            .into_iter()
            .map(|d| d.name)
            .collect();
        for expected in [
            "read_file",
            "list_dir",
            "grep",
            "git_status",
            "edit_file",
            "git_commit",
            "rollback",
        ] {
            assert!(
                no_egress.contains(&expected.to_string()),
                "missing {expected}"
            );
        }
        // Egress + ungoverned tools must be absent by default.
        for absent in ["git_push", "open_pr", "git_branch"] {
            assert!(
                !no_egress.contains(&absent.to_string()),
                "{absent} must not be exposed without opt-in"
            );
        }
        // With egress opted in, the two network tools appear.
        let with_egress: Vec<_> = coding_tool_descriptors(true)
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert!(with_egress.contains(&"git_push".to_string()));
        assert!(with_egress.contains(&"open_pr".to_string()));
        assert!(
            !with_egress.contains(&"git_branch".to_string()),
            "git_branch is ungoverned and must never be exposed"
        );
    }
}
