//! Integration tests for the Challenger middleware.
//!
//! Scenarios:
//! 1. Accept verdict  → step proceeds and succeeds as normal.
//! 2. Reject verdict  → step is skipped; `StepResult` records the critique.
//! 3. `RequestRevision` → planner re-invoked with critique context; second
//!    proposal is accepted; loop counter prevents infinite revision.
//! 4. Low-risk step   → challenger bypassed even when configured.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use xiaoguai_orchestrator::{
    budget::Budget,
    challenger::{Challenger, Critique, MockChallenger},
    plan::{PlanStep, RiskLevel},
    planner::Planner,
    OrchestratorError, RunOutcome, StepResult, Supervisor,
};

use common::{MockPlanner, MockWorker};

// ── helpers ───────────────────────────────────────────────────────────────────

fn high_step(id: &str, description: &str) -> PlanStep {
    PlanStep::new(id, description, vec![]).with_risk(RiskLevel::High)
}

fn low_step(id: &str) -> PlanStep {
    PlanStep::new(id, "low risk op", vec![])
}

// ── Test 1: Accept verdict → step proceeds ────────────────────────────────────

#[tokio::test]
async fn challenger_accept_step_proceeds() {
    let worker = MockWorker::always_ok("w", "payment drafted").push_ok("done");
    let planner = MockPlanner::new(vec![high_step("pay", "send $100 to vendor")]);
    let budget = Budget::new().with_max_steps(10);

    let challenger = Arc::new(MockChallenger::new().push(Critique::accept()));

    let mut sup = Supervisor::new(budget, Box::new(planner)).with_challenger(challenger);
    sup.add_worker(worker.clone());

    let report = sup.run_detailed("payment goal").await.unwrap();

    assert_eq!(report.outcome, RunOutcome::GoalAchieved);
    assert_eq!(report.history.len(), 1);
    assert!(
        report.history[0].success,
        "step should succeed after Accept"
    );
    assert_eq!(report.history[0].step_id, "pay");
    // No critique data on an accepted step that was dispatched.
    assert!(report.history[0].critique_reasons.is_none());
    assert_eq!(worker.call_count(), 1, "worker was called once");
}

// ── Test 2: Reject verdict → step skipped, critique recorded ─────────────────

#[tokio::test]
async fn challenger_reject_step_skipped_with_critique() {
    let worker = MockWorker::always_ok("w", "draft done").push_ok("unused");
    let planner = MockPlanner::new(vec![
        PlanStep::new("draft", "draft payment", vec![]),
        high_step("send", "send $100 to external account"),
    ]);
    let budget = Budget::new().with_max_steps(10);

    let challenger = Arc::new(MockChallenger::new()
            // "draft" is Low risk → bypassed; "send" is High risk → Reject
            .push(Critique::reject(
                vec![
                    "irreversible wire transfer".to_string(),
                    "recipient unverified".to_string(),
                ],
                0.95,
            )));

    let mut sup = Supervisor::new(budget, Box::new(planner)).with_challenger(challenger);
    sup.add_worker(worker.clone());

    let report = sup.run_detailed("payment workflow").await.unwrap();

    assert_eq!(report.outcome, RunOutcome::GoalAchieved);
    assert_eq!(report.history.len(), 2, "both steps in history");

    let draft_result = &report.history[0];
    assert_eq!(draft_result.step_id, "draft");
    assert!(draft_result.success);
    assert!(
        draft_result.critique_reasons.is_none(),
        "draft bypassed challenger"
    );

    let send_result = &report.history[1];
    assert_eq!(send_result.step_id, "send");
    assert!(!send_result.success, "send was skipped (not succeeded)");
    assert!(
        send_result.critique_reasons.is_some(),
        "critique reasons should be recorded"
    );
    let reasons = send_result.critique_reasons.as_ref().unwrap();
    assert!(reasons.iter().any(|r| r.contains("irreversible")));
    assert_eq!(send_result.critique_risk_score, Some(0.95));
    assert!(
        send_result.output.contains("Rejected by challenger"),
        "output should describe rejection: {}",
        send_result.output
    );

    // Worker was only called once (for "draft"); "send" was never dispatched.
    assert_eq!(
        worker.call_count(),
        1,
        "worker not called for rejected step"
    );
}

// ── Test 3: RequestRevision → planner re-invoked; revised step accepted ───────

/// A planner that emits one high-risk step on first call, then (after seeing
/// a revision-request marker in history) emits a revised safer step.
struct RevisionAwarePlanner {
    call_count: Mutex<u32>,
}

impl RevisionAwarePlanner {
    fn new() -> Self {
        Self {
            call_count: Mutex::new(0),
        }
    }
}

#[async_trait]
impl Planner for RevisionAwarePlanner {
    async fn next_step(
        &self,
        _goal: &str,
        history: &[StepResult],
    ) -> Result<Option<PlanStep>, OrchestratorError> {
        let mut count = self.call_count.lock().unwrap();
        *count += 1;

        // Check if we've been asked for a revision (critique marker in history).
        let has_revision_request = history.iter().any(|r| r.step_id.starts_with("__revision_"));

        if *count == 1 {
            // First call: emit the high-risk step.
            return Ok(Some(high_step("send-payment", "wire $500 immediately")));
        }

        if has_revision_request && *count == 2 {
            // After revision: emit a safer reformulated step.
            return Ok(Some(
                PlanStep::new(
                    "send-payment-revised",
                    "schedule $500 wire with 24h delay and dual approval",
                    vec![],
                )
                .with_risk(RiskLevel::Medium),
            ));
        }

        // Done.
        Ok(None)
    }
}

#[tokio::test]
async fn challenger_revision_planner_requeried_revised_step_accepted() {
    let worker = MockWorker::always_ok("w", "revised step done");
    let budget = Budget::new().with_max_steps(20);

    let challenger = Arc::new(
        MockChallenger::new()
            // First critique: RequestRevision.
            .push(Critique::revise(
                vec!["no approval gate".to_string(), "amount exceeds daily limit".to_string()],
                0.7,
            ))
            // Second critique: Accept (the revised step is Medium risk — bypassed,
            // but if it were High the MockChallenger would Accept).
            .push(Critique::accept()),
    );

    let planner = RevisionAwarePlanner::new();
    let mut sup = Supervisor::new(budget, Box::new(planner)).with_challenger(challenger);
    sup.add_worker(worker.clone());

    let report = sup.run_detailed("payment with revision").await.unwrap();

    assert_eq!(report.outcome, RunOutcome::GoalAchieved);
    // The revised step (Medium risk) was dispatched and succeeded.
    assert!(
        report
            .history
            .iter()
            .any(|r| r.step_id == "send-payment-revised"),
        "revised step should appear in history: {:?}",
        report
            .history
            .iter()
            .map(|r| &r.step_id)
            .collect::<Vec<_>>()
    );
    let revised = report
        .history
        .iter()
        .find(|r| r.step_id == "send-payment-revised")
        .unwrap();
    assert!(revised.success, "revised step should succeed");
    // Original high-risk step should NOT appear in history (it was swapped).
    assert!(
        !report.history.iter().any(|r| r.step_id == "send-payment"),
        "original step should not be dispatched after revision"
    );
    assert_eq!(
        worker.call_count(),
        1,
        "worker called exactly once for revised step"
    );
}

// ── Test 3b: Revision loop cap prevents infinite cycling ─────────────────────

/// A challenger that always returns `RequestRevision`.
struct AlwaysRevise;

#[async_trait]
impl Challenger for AlwaysRevise {
    async fn critique(&self, _proposed: &PlanStep) -> Result<Critique, OrchestratorError> {
        Ok(Critique::revise(vec!["always unhappy".to_string()], 0.5))
    }
}

/// A planner that keeps emitting the same high-risk step.
struct AlwaysHighPlanner;

#[async_trait]
impl Planner for AlwaysHighPlanner {
    async fn next_step(
        &self,
        _goal: &str,
        history: &[StepResult],
    ) -> Result<Option<PlanStep>, OrchestratorError> {
        // Stop after 1 real result in history (prevents true infinite loop in test).
        if history
            .iter()
            .any(|r| !r.step_id.starts_with("__revision_"))
        {
            return Ok(None);
        }
        Ok(Some(high_step("stubborn", "always risky")))
    }
}

#[tokio::test]
async fn challenger_revision_loop_capped_at_max_revisions() {
    let worker = MockWorker::always_ok("w", "forced through after cap");
    let budget = Budget::new().with_max_steps(20);

    let mut sup = Supervisor::new(budget, Box::new(AlwaysHighPlanner))
        .with_challenger(Arc::new(AlwaysRevise));
    sup.add_worker(worker.clone());

    // Should complete without hanging (the loop cap forces dispatch).
    let report = sup.run_detailed("stubborn goal").await.unwrap();
    assert_eq!(report.outcome, RunOutcome::GoalAchieved);

    // The step was eventually dispatched after hitting MAX_REVISIONS.
    assert!(
        report.history.iter().any(|r| r.step_id == "stubborn"),
        "step should be dispatched after revision cap"
    );
}

// ── Test 4: Low-risk step bypasses challenger ─────────────────────────────────

/// A challenger that panics if invoked — proves it is never called for low-risk steps.
struct PanicChallenger;

#[async_trait]
impl Challenger for PanicChallenger {
    async fn critique(&self, proposed: &PlanStep) -> Result<Critique, OrchestratorError> {
        panic!(
            "PanicChallenger should never be called, but was called for step '{}'",
            proposed.id
        );
    }
}

#[tokio::test]
async fn low_risk_step_bypasses_challenger() {
    let worker = MockWorker::always_ok("w", "safe done");
    let planner = MockPlanner::new(vec![low_step("read-data"), low_step("compute")]);
    let budget = Budget::new().with_max_steps(10);

    let mut sup =
        Supervisor::new(budget, Box::new(planner)).with_challenger(Arc::new(PanicChallenger));
    sup.add_worker(worker.clone());

    // Should complete without the PanicChallenger being invoked.
    let report = sup.run_detailed("safe workflow").await.unwrap();
    assert_eq!(report.outcome, RunOutcome::GoalAchieved);
    assert_eq!(worker.call_count(), 2, "both low-risk steps dispatched");
}

// ── Test 5: No challenger configured — high-risk step dispatches directly ─────

#[tokio::test]
async fn no_challenger_high_risk_step_dispatched_normally() {
    let worker = MockWorker::always_ok("w", "sent");
    let planner = MockPlanner::new(vec![high_step("wire", "wire funds")]);
    let budget = Budget::new().with_max_steps(10);

    // No .with_challenger() call.
    let mut sup = Supervisor::new(budget, Box::new(planner));
    sup.add_worker(worker.clone());

    let report = sup.run_detailed("no challenger").await.unwrap();
    assert_eq!(report.outcome, RunOutcome::GoalAchieved);
    assert_eq!(worker.call_count(), 1);
    assert!(report.history[0].success);
    assert!(report.history[0].critique_reasons.is_none());
}
