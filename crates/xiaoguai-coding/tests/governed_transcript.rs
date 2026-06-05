//! Coding eval (P0-d) — the **transcript grader**.
//!
//! Drives a realistic governed scenario end-to-end and asserts the full audit
//! transcript: every mutation produced exactly one audit row, each `[WRITE]`
//! mutation carries a checkpoint, a *denied* action mutates nothing and emits a
//! `*_denied` row, and the recorded checkpoint actually reverts the change.
//! This is what proves the workflow is *governed*, not merely *working* — the
//! same property a CI regression gate must protect.
//!
//! (The full `xiaoguai-eval` / `ReactAgent` binding additionally exercises the
//! LLM-driven dispatch; this transcript grader covers the governance contract
//! independently of any model.)

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use xiaoguai_coding::{
    CodingGate, CodingStep, FileEdit, GateDecision, GovernedTools, StepRecorder, Workspace,
};

/// Gate that allows everything except scopes in its deny-list.
struct PolicyGate {
    deny: Vec<String>,
}
#[async_trait]
impl CodingGate for PolicyGate {
    async fn decide(&self, scope: &str) -> GateDecision {
        if self.deny.iter().any(|s| s == scope) {
            GateDecision::Deny(format!("policy denies {scope}"))
        } else {
            GateDecision::Allow
        }
    }
}

/// Cloneable, Arc-backed recorder so the test can read the transcript after the
/// `GovernedTools` has taken ownership of its clone.
#[derive(Clone, Default)]
struct Transcript(Arc<Mutex<Vec<CodingStep>>>);
#[async_trait]
impl StepRecorder for Transcript {
    async fn record(&self, step: CodingStep) {
        self.0.lock().unwrap().push(step);
    }
}
impl Transcript {
    fn steps(&self) -> Vec<CodingStep> {
        self.0.lock().unwrap().clone()
    }
}

#[tokio::test]
async fn governed_scenario_produces_a_complete_audited_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::open_or_create(dir.path()).await.unwrap();
    let transcript = Transcript::default();
    // Deny only commits, to exercise the denied path mid-scenario.
    let tools = GovernedTools::new(
        ws,
        PolicyGate {
            deny: vec!["tool_call.git_commit".to_string()],
        },
        transcript.clone(),
    );

    // 1) edit two files (allowed) → two code.edit rows, each checkpointed
    let e1 = tools
        .edit_file(
            Path::new("src/a.rs"),
            &FileEdit::Write("fn a() {}\n".into()),
        )
        .await
        .expect("edit a");
    tools
        .edit_file(
            Path::new("src/b.rs"),
            &FileEdit::Write("fn b() {}\n".into()),
        )
        .await
        .expect("edit b");

    // 2) a denied commit → no commit, a git.commit_denied row, tree unchanged
    let denied = tools.git_commit("should be blocked").await;
    assert!(denied.is_err(), "commit must be denied by policy");

    // 3) rollback to the first checkpoint (pre-a) → both files vanish
    tools.rollback(&e1.checkpoint).await.expect("rollback");

    // --- grade the transcript ---------------------------------------------
    let steps = transcript.steps();
    let actions: Vec<&str> = steps.iter().map(|s| s.action.as_str()).collect();
    assert_eq!(
        actions,
        vec![
            "code.edit",         // a.rs
            "code.edit",         // b.rs
            "git.commit_denied", // blocked
            "code.rollback",     // restore
        ],
        "unexpected audit transcript"
    );

    // Every non-denied [WRITE] step carries a checkpoint; the denied one does not.
    assert!(steps[0].checkpoint.is_some(), "edit a must checkpoint");
    assert!(steps[1].checkpoint.is_some(), "edit b must checkpoint");
    assert!(
        steps[2].checkpoint.is_none(),
        "denied commit must not checkpoint"
    );
    assert!(steps[3].checkpoint.is_some(), "rollback records its target");

    // Outcome: the denied commit did not run, and rollback reverted the tree.
    assert!(
        tools
            .workspace()
            .read_file(Path::new("src/a.rs"))
            .await
            .is_err(),
        "rollback should have removed src/a.rs"
    );
    assert!(
        tools
            .workspace()
            .read_file(Path::new("src/b.rs"))
            .await
            .is_err(),
        "rollback should have removed src/b.rs"
    );
}
