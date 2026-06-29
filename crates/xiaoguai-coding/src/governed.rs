//! Governed dispatch (DEC-034 / DEC-035, `LLD-CODING-001` §4.1).
//!
//! [`GovernedTools`] ties the coding operations ([`crate::tools`]) to the
//! moat: every mutating action goes **gate → checkpoint → mutate → audit**, and
//! a denied action goes **abort → audit-denied** with no mutation. The `HotL`
//! gate and audit chain are reached through the [`CodingGate`] and
//! [`StepRecorder`] traits so this crate stays decoupled — `xiaoguai-core`
//! implements them over the real `HotlGate` (DEC-006) and `SqliteAuditSink`
//! (DEC-004), and tests use mocks.
//!
//! Note the split of responsibility (per DEC-006): the agent loop owns the
//! `Suspend`/resume lifecycle (`DecisionRegistry` + SSE). By the time a verdict
//! reaches [`CodingGate`] it is already resolved to [`GateDecision::Allow`] or
//! [`GateDecision::Deny`]; the checkpoint is therefore taken **after** any
//! approval, immediately before the mutation, exactly as DEC-035 requires.

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use xiaoguai_mcp_exec_js::{run_javascript, ExecConfig, ExecResult};

use crate::checkpoint::CheckpointId;
use crate::error::CodingError;
use crate::tools::{EditSummary, FileEdit};
use crate::workspace::{CommandRun, Workspace};

/// Length cap on the command string echoed into a `code.exec` audit summary,
/// so a long one-liner doesn't bloat the audit row (the full command is the
/// model's tool arg, already on the chain via the surrounding turn).
const EXEC_SUMMARY_CMD_CAP: usize = 200;

/// Per-call wall-clock deadline requested for a sandboxed `run_code` snippet.
/// Clamped down by `ExecConfig::max_timeout` (30s) inside `run_javascript`, so
/// this is the effective ceiling for a JS snippet.
const RUN_CODE_TIMEOUT: Duration = Duration::from_secs(30);

/// A resolved gate decision for one mutating coding action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// Proceed with the mutation.
    Allow,
    /// Refuse the mutation; the string is a human reason for the audit row.
    Deny(String),
}

/// Gate a single coding action by its `tool_call.<verb>` scope. Implemented by
/// `xiaoguai-core` over the real `HotL` gate (after resolving any suspension).
#[async_trait]
pub trait CodingGate: Send + Sync {
    async fn decide(&self, scope: &str) -> GateDecision;
}

/// One audited coding step. `xiaoguai-core` maps this onto an `AuditEntry` and
/// appends it to the HMAC chain; the `checkpoint` links "what changed" to "how
/// to revert it" (DEC-035).
#[derive(Debug, Clone)]
pub struct CodingStep {
    /// Canonical action name from `LLD-CODING-001` §2 (e.g. `code.edit`,
    /// `code.edit_denied`, `git.commit`).
    pub action: String,
    pub workspace_id: String,
    pub scope: String,
    pub checkpoint: Option<String>,
    pub summary: String,
}

/// Append a [`CodingStep`] to the audit chain. Implementations must not block
/// the operation on an audit-write failure (degrade to a warning), per the
/// project's audit-resilience rule.
#[async_trait]
pub trait StepRecorder: Send + Sync {
    async fn record(&self, step: CodingStep);
}

/// The result of a successful governed mutation: the checkpoint taken just
/// before it (for rollback) and the operation's own result.
#[derive(Debug, Clone)]
pub struct GovernedOutcome<T> {
    pub checkpoint: CheckpointId,
    pub result: T,
}

/// Wraps a [`Workspace`] with a gate + recorder and exposes the coding tools as
/// governed actions. Read operations pass straight through (no gate/checkpoint
/// per the canonical table); mutations follow the full sequence.
#[derive(Debug)]
pub struct GovernedTools<G, R> {
    workspace: Workspace,
    gate: G,
    recorder: R,
}

impl<G: CodingGate, R: StepRecorder> GovernedTools<G, R> {
    pub fn new(workspace: Workspace, gate: G, recorder: R) -> Self {
        Self {
            workspace,
            gate,
            recorder,
        }
    }

    #[must_use]
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// Gate one action. On deny, records `<action>_denied` and returns
    /// [`CodingError::Denied`] so the caller never proceeds.
    async fn authorize(&self, scope: &str, action: &str) -> Result<(), CodingError> {
        match self.gate.decide(scope).await {
            GateDecision::Allow => Ok(()),
            GateDecision::Deny(reason) => {
                self.recorder
                    .record(CodingStep {
                        action: format!("{action}_denied"),
                        workspace_id: self.workspace.id().to_string(),
                        scope: scope.to_string(),
                        checkpoint: None,
                        summary: reason.clone(),
                    })
                    .await;
                Err(CodingError::Denied {
                    scope: scope.to_string(),
                    reason,
                })
            }
        }
    }

    /// Gate + checkpoint. On allow, returns the checkpoint id taken immediately
    /// before the mutation; on deny, behaves like [`Self::authorize`].
    async fn begin(
        &self,
        scope: &str,
        action: &str,
        label: &str,
    ) -> Result<CheckpointId, CodingError> {
        self.authorize(scope, action).await?;
        self.workspace.checkpoint(label).await
    }

    /// Record an egress action (`git.push` / `pr.open`) that does not mutate the
    /// worktree, so carries no checkpoint — it is the "past local undo" boundary.
    async fn record_egress(&self, action: &str, scope: &str, summary: String) {
        self.recorder
            .record(CodingStep {
                action: action.to_string(),
                workspace_id: self.workspace.id().to_string(),
                scope: scope.to_string(),
                checkpoint: None,
                summary,
            })
            .await;
    }

    /// Record a successful mutation against its checkpoint, then snapshot the
    /// **post**-mutation tree so the checkpoint chain's tip reflects what the
    /// agent just changed. This is what lets `rollback` prune the agent's
    /// additions (they sit between the pre-edit checkpoint and the tip) while
    /// leaving files the user created out-of-band — captured by no checkpoint —
    /// untouched. Best-effort: a failed post-snapshot only narrows what a later
    /// rollback can prune, it never fails the mutation that already succeeded.
    async fn finish(&self, action: &str, scope: &str, checkpoint: &CheckpointId, summary: String) {
        self.recorder
            .record(CodingStep {
                action: action.to_string(),
                workspace_id: self.workspace.id().to_string(),
                scope: scope.to_string(),
                checkpoint: Some(checkpoint.to_string()),
                summary,
            })
            .await;
        if let Err(err) = self.workspace.checkpoint(&format!("post:{action}")).await {
            tracing::warn!(
                %action, %err,
                "post-mutation checkpoint failed; rollback may not prune this edit's additions"
            );
        }
    }

    /// Governed `edit_file`: gate `tool_call.edit_file` → checkpoint → apply →
    /// audit `code.edit`.
    pub async fn edit_file(
        &self,
        rel: &Path,
        edit: &FileEdit,
    ) -> Result<GovernedOutcome<EditSummary>, CodingError> {
        let cp = self
            .begin("tool_call.edit_file", "code.edit", "pre:edit_file")
            .await?;
        let result = self.workspace.edit_file(rel, edit).await?;
        self.finish(
            "code.edit",
            "tool_call.edit_file",
            &cp,
            format!("{} (+{} repl)", result.path.display(), result.replacements),
        )
        .await;
        Ok(GovernedOutcome {
            checkpoint: cp,
            result,
        })
    }

    /// Governed `git_commit`: gate `tool_call.git_commit` → checkpoint → commit
    /// → audit `git.commit`. Returns the new commit SHA.
    pub async fn git_commit(&self, message: &str) -> Result<GovernedOutcome<String>, CodingError> {
        let cp = self
            .begin("tool_call.git_commit", "git.commit", "pre:git_commit")
            .await?;
        let sha = self.workspace.git_commit(message).await?;
        self.finish(
            "git.commit",
            "tool_call.git_commit",
            &cp,
            format!("commit {} — {message}", short(&sha)),
        )
        .await;
        Ok(GovernedOutcome {
            checkpoint: cp,
            result: sha,
        })
    }

    /// Governed `rollback`: gate `tool_call.rollback` → restore the worktree to
    /// `to` → audit `code.rollback`. Unlike a forward mutation it takes no fresh
    /// checkpoint; the audit row's checkpoint is the rollback *target*.
    pub async fn rollback(&self, to: &CheckpointId) -> Result<(), CodingError> {
        match self.gate.decide("tool_call.rollback").await {
            GateDecision::Allow => {
                self.workspace.rollback(to).await?;
                self.finish(
                    "code.rollback",
                    "tool_call.rollback",
                    to,
                    format!("rolled back to {to}"),
                )
                .await;
                Ok(())
            }
            GateDecision::Deny(reason) => {
                self.recorder
                    .record(CodingStep {
                        action: "code.rollback_denied".to_string(),
                        workspace_id: self.workspace.id().to_string(),
                        scope: "tool_call.rollback".to_string(),
                        checkpoint: Some(to.to_string()),
                        summary: reason.clone(),
                    })
                    .await;
                Err(CodingError::Denied {
                    scope: "tool_call.rollback".to_string(),
                    reason,
                })
            }
        }
    }

    /// Governed `git_push`: gate `tool_call.git_push` → push → audit `git.push`.
    /// No checkpoint — push does not change the worktree and is the egress
    /// (past-local-undo) boundary.
    pub async fn git_push(&self, remote: &str, branch: &str) -> Result<String, CodingError> {
        self.authorize("tool_call.git_push", "git.push").await?;
        let out = self.workspace.git_push(remote, branch).await?;
        self.record_egress(
            "git.push",
            "tool_call.git_push",
            format!("{remote} {branch}"),
        )
        .await;
        Ok(out)
    }

    /// Governed `open_pr`: gate `tool_call.open_pr` → `gh pr create` → audit
    /// `pr.open`. Egress; no checkpoint. Returns the PR URL.
    pub async fn open_pr(
        &self,
        title: &str,
        body: &str,
        base: &str,
    ) -> Result<String, CodingError> {
        self.authorize("tool_call.open_pr", "pr.open").await?;
        let url = self.workspace.open_pr(title, body, base).await?;
        self.record_egress("pr.open", "tool_call.open_pr", format!("{title} → {url}"))
            .await;
        Ok(url)
    }

    /// Governed `run_command`: gate `tool_call.run_command` → run the shell
    /// command in the workspace root → audit `code.exec`. No checkpoint —
    /// process side effects are not reversible by the worktree checkpoint chain
    /// (like `git_push`), so exec is recorded as a non-revertible step. On deny
    /// it records `code.exec_denied` and proceeds no further.
    pub async fn run_command(&self, command: &str) -> Result<CommandRun, CodingError> {
        self.authorize("tool_call.run_command", "code.exec").await?;
        let run = self.workspace.run_command(command).await?;
        let outcome = if run.timed_out {
            "timed out".to_string()
        } else {
            format!("exit={}", exit_label(run.exit_code))
        };
        self.record_egress(
            "code.exec",
            "tool_call.run_command",
            format!("{} ({outcome})", truncate_command(command)),
        )
        .await;
        Ok(run)
    }

    /// Governed `run_code`: gate `tool_call.run_code` → run the JavaScript
    /// snippet in an ISOLATED sandbox (fresh tempdir CWD + scrubbed env +
    /// `ulimit` + wall-clock deadline, all inside
    /// [`xiaoguai_mcp_exec_js::run_javascript`]) → audit `code.exec_sandboxed`.
    ///
    /// This is the lower-risk sibling of [`Self::run_command`]: the sandbox
    /// cannot touch the session working dir or the owner's files, so there is
    /// nothing to checkpoint — like the other exec/egress steps it is recorded
    /// as a non-revertible action. On deny it records `code.exec_sandboxed_denied`
    /// and proceeds no further.
    ///
    /// A *crashing* snippet (non-zero exit, or a timeout) is a successful
    /// supervision and comes back in the [`ExecResult`]; only the supervisor
    /// itself failing (runtime missing, snippet over cap, spawn/IO error) maps
    /// to [`CodingError::Exec`].
    pub async fn run_code(&self, code: &str) -> Result<ExecResult, CodingError> {
        self.authorize("tool_call.run_code", "code.exec_sandboxed")
            .await?;
        let result = run_javascript(&ExecConfig::default(), code, RUN_CODE_TIMEOUT)
            .await
            .map_err(|e| CodingError::Exec {
                reason: e.to_string(),
            })?;
        let outcome = if result.timed_out {
            "timed out".to_string()
        } else {
            format!("exit={}", exit_label(result.exit_code))
        };
        self.record_egress(
            "code.exec_sandboxed",
            "tool_call.run_code",
            format!("{} ({outcome})", truncate_command(code)),
        )
        .await;
        Ok(result)
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

/// Render an exit code for an audit summary; `None` (signalled/killed) shows
/// as `signal` since no numeric code is available.
fn exit_label(code: Option<i32>) -> String {
    match code {
        Some(c) => c.to_string(),
        None => "signal".to_string(),
    }
}

/// Truncate a command to [`EXEC_SUMMARY_CMD_CAP`] chars (on a char boundary)
/// for the audit summary, marking truncation with an ellipsis.
fn truncate_command(command: &str) -> String {
    if command.chars().count() <= EXEC_SUMMARY_CMD_CAP {
        return command.to_string();
    }
    let mut out: String = command.chars().take(EXEC_SUMMARY_CMD_CAP).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Gate that allows or denies every action with a fixed verdict.
    struct FixedGate(GateDecision);
    #[async_trait]
    impl CodingGate for FixedGate {
        async fn decide(&self, _scope: &str) -> GateDecision {
            self.0.clone()
        }
    }

    /// Records every step it is handed, for assertions.
    #[derive(Default)]
    struct SpyRecorder(Mutex<Vec<CodingStep>>);
    #[async_trait]
    impl StepRecorder for SpyRecorder {
        async fn record(&self, step: CodingStep) {
            self.0.lock().unwrap().push(step);
        }
    }
    impl SpyRecorder {
        fn actions(&self) -> Vec<String> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .map(|s| s.action.clone())
                .collect()
        }
        fn last_checkpoint(&self) -> Option<String> {
            self.0
                .lock()
                .unwrap()
                .last()
                .and_then(|s| s.checkpoint.clone())
        }
    }

    async fn workspace() -> (tempfile::TempDir, Workspace) {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::open_or_create(dir.path()).await.unwrap();
        (dir, ws)
    }

    #[tokio::test]
    async fn allowed_edit_checkpoints_mutates_and_audits() {
        let (_dir, ws) = workspace().await;
        let tools = GovernedTools::new(ws, FixedGate(GateDecision::Allow), SpyRecorder::default());

        let out = tools
            .edit_file(Path::new("a.txt"), &FileEdit::Write("hello".into()))
            .await
            .unwrap();

        // file written
        assert_eq!(
            tools
                .workspace()
                .read_file(Path::new("a.txt"))
                .await
                .unwrap(),
            "hello"
        );
        // exactly one audit row, `code.edit`, carrying the checkpoint id
        assert_eq!(tools.recorder.actions(), vec!["code.edit"]);
        assert_eq!(
            tools.recorder.last_checkpoint().as_deref(),
            Some(out.checkpoint.as_str())
        );
        // the checkpoint was taken BEFORE the edit (DEC-035), so rolling back to
        // it undoes the edit — a.txt did not exist pre-edit, so it is removed.
        tools.workspace().rollback(&out.checkpoint).await.unwrap();
        assert!(tools
            .workspace()
            .read_file(Path::new("a.txt"))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn denied_edit_does_not_mutate_and_audits_denied() {
        let (_dir, ws) = workspace().await;
        let tools = GovernedTools::new(
            ws,
            FixedGate(GateDecision::Deny("no edits in prod".into())),
            SpyRecorder::default(),
        );

        let err = tools
            .edit_file(Path::new("a.txt"), &FileEdit::Write("hello".into()))
            .await
            .unwrap_err();

        assert!(matches!(err, CodingError::Denied { .. }));
        // no file created
        assert!(tools
            .workspace()
            .read_file(Path::new("a.txt"))
            .await
            .is_err());
        // a single denied audit row, no checkpoint
        assert_eq!(tools.recorder.actions(), vec!["code.edit_denied"]);
        assert_eq!(tools.recorder.last_checkpoint(), None);
    }

    #[tokio::test]
    async fn governed_rollback_restores_and_audits() {
        let (_dir, ws) = workspace().await;
        let tools = GovernedTools::new(ws, FixedGate(GateDecision::Allow), SpyRecorder::default());
        let first = tools
            .edit_file(Path::new("a.txt"), &FileEdit::Write("one".into()))
            .await
            .unwrap();
        tools
            .edit_file(Path::new("a.txt"), &FileEdit::Write("two".into()))
            .await
            .unwrap();

        tools.rollback(&first.checkpoint).await.unwrap();

        // restored to the pre-"one" state (a.txt did not exist there)
        assert!(tools
            .workspace()
            .read_file(Path::new("a.txt"))
            .await
            .is_err());
        assert_eq!(
            tools.recorder.actions(),
            vec!["code.edit", "code.edit", "code.rollback"]
        );
    }

    #[tokio::test]
    async fn governed_push_to_local_bare_remote_audits_git_push() {
        // Egress is verifiable without GitHub: push to a local bare repo.
        let bare = tempfile::tempdir().unwrap();
        crate::git::run(bare.path(), &["init", "--bare", "-q"], None)
            .await
            .unwrap();

        let (_dir, ws) = workspace().await;
        crate::git::run(
            ws.root(),
            &["remote", "add", "origin", &bare.path().to_string_lossy()],
            None,
        )
        .await
        .unwrap();
        let tools = GovernedTools::new(ws, FixedGate(GateDecision::Allow), SpyRecorder::default());
        tools
            .edit_file(Path::new("f.txt"), &FileEdit::Write("x".into()))
            .await
            .unwrap();
        tools.git_commit("init").await.unwrap();

        // determine the current branch name (init.defaultBranch varies)
        let branch = crate::git::run(
            tools.workspace().root(),
            &["branch", "--show-current"],
            None,
        )
        .await
        .unwrap();
        tools.git_push("origin", &branch).await.unwrap();

        // the bare remote now has the branch, and a git.push row was audited (no checkpoint)
        let remote_branches = crate::git::run(bare.path(), &["branch", "--list"], None)
            .await
            .unwrap();
        assert!(remote_branches.contains(branch.trim()));
        assert!(tools.recorder.actions().contains(&"git.push".to_string()));
    }

    #[tokio::test]
    async fn denied_push_does_not_push_and_audits_denied() {
        let (_dir, ws) = workspace().await;
        let tools = GovernedTools::new(
            ws,
            FixedGate(GateDecision::Deny("no egress".into())),
            SpyRecorder::default(),
        );
        let err = tools.git_push("origin", "main").await.unwrap_err();
        assert!(matches!(err, CodingError::Denied { .. }));
        assert_eq!(tools.recorder.actions(), vec!["git.push_denied"]);
    }

    #[tokio::test]
    async fn allowed_commit_audits_git_commit() {
        let (_dir, ws) = workspace().await;
        let tools = GovernedTools::new(ws, FixedGate(GateDecision::Allow), SpyRecorder::default());
        tools
            .edit_file(Path::new("f.txt"), &FileEdit::Write("x".into()))
            .await
            .unwrap();
        let out = tools.git_commit("init").await.unwrap();
        assert_eq!(out.result.len(), 40); // full SHA
        assert_eq!(tools.recorder.actions(), vec!["code.edit", "git.commit"]);
    }

    #[tokio::test]
    async fn allowed_run_command_runs_and_audits_code_exec() {
        let (_dir, ws) = workspace().await;
        let tools = GovernedTools::new(ws, FixedGate(GateDecision::Allow), SpyRecorder::default());
        let run = tools.run_command("echo hi").await.unwrap();
        assert!(run.stdout_combined.contains("hi"), "{run:?}");
        assert_eq!(run.exit_code, Some(0));
        // a single `code.exec` row, no checkpoint (exec is not reversible).
        assert_eq!(tools.recorder.actions(), vec!["code.exec"]);
        assert_eq!(tools.recorder.last_checkpoint(), None);
    }

    #[tokio::test]
    async fn denied_run_command_does_not_run_and_audits_denied() {
        let (_dir, ws) = workspace().await;
        let tools = GovernedTools::new(
            ws,
            FixedGate(GateDecision::Deny("consult mode: no exec".into())),
            SpyRecorder::default(),
        );
        // A command that would leave a side effect if it ran.
        let err = tools
            .run_command("echo ran > should-not-exist.txt")
            .await
            .unwrap_err();
        assert!(matches!(err, CodingError::Denied { .. }));
        assert!(
            !tools
                .workspace()
                .root()
                .join("should-not-exist.txt")
                .exists(),
            "denied command must not run"
        );
        assert_eq!(tools.recorder.actions(), vec!["code.exec_denied"]);
        assert_eq!(tools.recorder.last_checkpoint(), None);
    }

    #[tokio::test]
    async fn denied_run_code_does_not_exec_and_audits_denied() {
        // Deny path needs no JS runtime: the gate refuses BEFORE run_javascript
        // is ever called, so this is safe to run un-ignored in CI (cf. #243,
        // which quarantined deno-spawning tests for runner death).
        let (_dir, ws) = workspace().await;
        let tools = GovernedTools::new(
            ws,
            FixedGate(GateDecision::Deny("consult mode: no sandboxed exec".into())),
            SpyRecorder::default(),
        );
        let err = tools
            .run_code("console.log('should never run')")
            .await
            .unwrap_err();
        assert!(matches!(err, CodingError::Denied { .. }));
        // exactly one denied row under the sandboxed action name, no checkpoint.
        assert_eq!(tools.recorder.actions(), vec!["code.exec_sandboxed_denied"]);
        assert_eq!(tools.recorder.last_checkpoint(), None);
    }

    /// Allow-path smoke for `run_code`: ACTUALLY spawns deno, so it is
    /// `#[ignore]`d to keep it out of the default CI path (#243 — deno-spawning
    /// tests caused runner OOM/death in xiaoguai-mcp-exec). Run locally with
    /// `cargo test -p xiaoguai-coding -- --ignored run_code_allow` on a box with
    /// deno on PATH.
    #[tokio::test]
    #[ignore = "spawns deno; gated out of CI per #243 runner-death"]
    async fn allowed_run_code_executes_in_sandbox_and_audits() {
        let (_dir, ws) = workspace().await;
        let tools = GovernedTools::new(ws, FixedGate(GateDecision::Allow), SpyRecorder::default());
        let result = tools
            .run_code("console.log('hello from sandbox')")
            .await
            .expect("supervisor must succeed");
        assert_eq!(result.exit_code, Some(0), "stderr: {}", result.stderr);
        assert_eq!(result.stdout.trim(), "hello from sandbox");
        assert!(!result.timed_out);
        // one non-revertible sandboxed-exec row, no checkpoint.
        assert_eq!(tools.recorder.actions(), vec!["code.exec_sandboxed"]);
        assert_eq!(tools.recorder.last_checkpoint(), None);
    }

    #[test]
    fn truncate_command_caps_long_commands() {
        let short = "echo hi";
        assert_eq!(truncate_command(short), short);
        let long = "x".repeat(EXEC_SUMMARY_CMD_CAP + 50);
        let out = truncate_command(&long);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), EXEC_SUMMARY_CMD_CAP + 1);
    }
}
