//! Supervisor stops when `Budget::max_steps` is reached even if the planner
//! keeps proposing new steps.

mod common;

use xiaoguai_orchestrator::{budget::Budget, plan::PlanStep, RunOutcome, Supervisor};

use common::MockWorker;

/// A planner that never says "done" — always emits a new step.
struct InfinitePlanner;

#[async_trait::async_trait]
impl xiaoguai_orchestrator::planner::Planner for InfinitePlanner {
    async fn next_step(
        &self,
        _goal: &str,
        history: &[xiaoguai_orchestrator::StepResult],
    ) -> Result<
        Option<xiaoguai_orchestrator::plan::PlanStep>,
        xiaoguai_orchestrator::OrchestratorError,
    > {
        let n = history.len();
        Ok(Some(PlanStep::new(
            format!("step-{n}"),
            format!("task {n}"),
            vec![],
        )))
    }
}

#[tokio::test]
async fn stops_at_max_steps() {
    let worker = MockWorker::always_ok("w", "done");
    let planner = InfinitePlanner;
    let budget = Budget::new().with_max_steps(3);

    let mut supervisor = Supervisor::new(budget, Box::new(planner));
    supervisor.add_worker(worker.clone());

    let outcome = supervisor.run("endless goal").await.expect("run ok");

    assert_eq!(
        outcome,
        RunOutcome::BudgetExhausted,
        "expected BudgetExhausted, got {outcome:?}"
    );
    // Exactly 3 steps dispatched.
    assert_eq!(worker.call_count(), 3);
}
