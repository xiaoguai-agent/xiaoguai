//! Supervisor stops cleanly when the planner returns `None` (goal met).

mod common;

use xiaoguai_orchestrator::{budget::Budget, RunOutcome, Supervisor};

use common::{MockPlanner, MockWorker};

#[tokio::test]
async fn terminates_when_planner_returns_none() {
    // Three-step plan; planner goes silent after step 2.
    let worker = MockWorker::always_ok("w", "ok")
        .push_ok("ok2")
        .push_ok("ok3");
    let planner = MockPlanner::linear(3);
    let budget = Budget::new().with_max_steps(100); // far above 3

    let mut supervisor = Supervisor::new(budget, Box::new(planner));
    supervisor.add_worker(worker.clone());

    let outcome = supervisor.run("three-step goal").await.expect("run ok");

    assert_eq!(
        outcome,
        RunOutcome::GoalAchieved,
        "expected GoalAchieved, got {outcome:?}"
    );
    assert_eq!(worker.call_count(), 3, "all three steps dispatched");
}

#[tokio::test]
async fn terminates_immediately_for_empty_plan() {
    let worker = MockWorker::always_ok("w", "ok");
    let planner = MockPlanner::new(vec![]); // no steps at all
    let budget = Budget::new().with_max_steps(10);

    let mut supervisor = Supervisor::new(budget, Box::new(planner));
    supervisor.add_worker(worker.clone());

    let outcome = supervisor.run("trivial goal").await.expect("run ok");

    assert_eq!(outcome, RunOutcome::GoalAchieved);
    assert_eq!(worker.call_count(), 0);
}
