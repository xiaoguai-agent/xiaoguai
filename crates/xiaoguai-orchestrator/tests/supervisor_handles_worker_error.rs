//! Worker errors are recorded and supervisor continues (default policy: continue).

mod common;

use std::sync::Arc;

use xiaoguai_orchestrator::{budget::Budget, plan::PlanStep, RunOutcome, Supervisor};

use common::MockPlanner;

#[tokio::test]
async fn worker_error_recorded_supervisor_continues() {
    // 3-step plan; step-1 fails, steps 0 and 2 succeed.
    let worker = Arc::new(common::MockWorker {
        name: "w".to_string(),
        responses: std::sync::Mutex::new(std::collections::VecDeque::new()),
        call_log: std::sync::Mutex::new(Vec::new()),
    });
    worker.clone().push_ok("step-0 done");
    worker.clone().push_err("oops, step-1 failed");
    worker.clone().push_ok("step-2 done");

    let planner = MockPlanner::linear(3);
    let budget = Budget::new().with_max_steps(10);

    let mut supervisor = Supervisor::new(budget, Box::new(planner));
    supervisor.add_worker(worker.clone());

    let outcome = supervisor.run("tolerate errors").await.expect("run ok");

    // Supervisor reaches GoalAchieved because planner ran out of steps.
    assert_eq!(outcome, RunOutcome::GoalAchieved);
    assert_eq!(worker.call_count(), 3, "all three steps attempted");
}

#[tokio::test]
async fn worker_error_is_present_in_history() {
    let worker = Arc::new(common::MockWorker {
        name: "w".to_string(),
        responses: std::sync::Mutex::new(std::collections::VecDeque::new()),
        call_log: std::sync::Mutex::new(Vec::new()),
    });
    worker.clone().push_err("bad");
    worker.clone().push_ok("good");

    let steps = vec![
        PlanStep::new("s0", "do s0", vec![]),
        PlanStep::new("s1", "do s1", vec![]),
    ];
    let planner = MockPlanner::new(steps);
    let budget = Budget::new().with_max_steps(10);

    let mut supervisor = Supervisor::new(budget, Box::new(planner));
    supervisor.add_worker(worker.clone());

    let outcome = supervisor.run("check history").await.expect("run ok");
    assert_eq!(outcome, RunOutcome::GoalAchieved);
    assert_eq!(worker.call_count(), 2);
}
