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

use async_trait::async_trait;

use crate::checkpoint::CheckpointId;
use crate::error::CodingError;
use crate::tools::{EditSummary, FileEdit};
use crate::workspace::Workspace;

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

    /// Gate + checkpoint. On allow, returns the checkpoint id taken immediately
    /// before the mutation. On deny, records `<action>_denied` and returns
    /// [`CodingError::Denied`] — the caller never mutates.
    async fn begin(
        &self,
        scope: &str,
        action: &str,
        label: &str,
    ) -> Result<CheckpointId, CodingError> {
        match self.gate.decide(scope).await {
            GateDecision::Allow => self.workspace.checkpoint(label).await,
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

    /// Record a successful mutation against its checkpoint.
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
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
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
}
