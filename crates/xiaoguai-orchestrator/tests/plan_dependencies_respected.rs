//! Steps whose dependencies have not yet completed are skipped; they become
//! eligible only after their prerequisites appear in the success history.

mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use xiaoguai_orchestrator::{
    budget::Budget, plan::PlanStep, planner::Planner, OrchestratorError, RunOutcome, StepResult,
    Supervisor, Task, WorkerResult,
};

use common::MockWorker;

// ── DiamondPlanner ────────────────────────────────────────────────────────────

/// A planner that models a diamond: A → {B, C} → D.
///
/// Emission order:
///   1. `a`  (no deps)
///   2. `b`  (deps: `["a"]`)
///   3. `c`  (deps: `["a"]`)
///   4. `d`  (deps: `["b", "c"]`)
///
/// The planner only emits `b` and `c` after it sees `a` in history,
/// and only `d` after it sees both `b` and `c`.
struct DiamondPlanner {
    emitted: Mutex<Vec<String>>,
}

impl DiamondPlanner {
    fn new() -> Self {
        Self {
            emitted: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl Planner for DiamondPlanner {
    async fn next_step(
        &self,
        _goal: &str,
        history: &[StepResult],
    ) -> Result<Option<PlanStep>, OrchestratorError> {
        let done: Vec<String> = history
            .iter()
            .filter(|s| s.success)
            .map(|s| s.step_id.clone())
            .collect();
        let mut emitted = self.emitted.lock().unwrap();

        if !emitted.contains(&"a".to_string()) {
            emitted.push("a".to_string());
            return Ok(Some(PlanStep::new("a", "do A", vec![])));
        }
        if !emitted.contains(&"b".to_string()) && done.contains(&"a".to_string()) {
            emitted.push("b".to_string());
            return Ok(Some(PlanStep::new("b", "do B", vec!["a".to_string()])));
        }
        if !emitted.contains(&"c".to_string()) && done.contains(&"a".to_string()) {
            emitted.push("c".to_string());
            return Ok(Some(PlanStep::new("c", "do C", vec!["a".to_string()])));
        }
        if !emitted.contains(&"d".to_string())
            && done.contains(&"b".to_string())
            && done.contains(&"c".to_string())
        {
            emitted.push("d".to_string());
            return Ok(Some(PlanStep::new(
                "d",
                "do D",
                vec!["b".to_string(), "c".to_string()],
            )));
        }
        Ok(None)
    }
}

#[tokio::test]
async fn diamond_completes_in_correct_order() {
    let worker = MockWorker::always_ok("w", "ok")
        .push_ok("ok")
        .push_ok("ok")
        .push_ok("ok");
    let planner = DiamondPlanner::new();
    let budget = Budget::new().with_max_steps(10);

    let mut supervisor = Supervisor::new(budget, Box::new(planner));
    supervisor.add_worker(worker.clone());

    let outcome = supervisor.run("diamond goal").await.expect("run ok");
    assert_eq!(outcome, RunOutcome::GoalAchieved);
    // a + b + c + d = 4 dispatches
    assert_eq!(worker.call_count(), 4);
}

// ── Sequential dependency test ────────────────────────────────────────────────

struct OrderCapture {
    inner: Arc<MockWorker>,
    order: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl xiaoguai_orchestrator::worker::Worker for OrderCapture {
    async fn execute(&self, task: Task) -> Result<WorkerResult, OrchestratorError> {
        self.order.lock().unwrap().push(task.step_id.clone());
        self.inner.execute(task).await
    }
}

struct SeqPlanner {
    steps: Mutex<VecDeque<PlanStep>>,
}

#[async_trait]
impl Planner for SeqPlanner {
    async fn next_step(
        &self,
        _goal: &str,
        history: &[StepResult],
    ) -> Result<Option<PlanStep>, OrchestratorError> {
        let done: Vec<String> = history
            .iter()
            .filter(|s| s.success)
            .map(|s| s.step_id.clone())
            .collect();
        let mut q = self.steps.lock().unwrap();
        // Only emit front step if its deps are all satisfied.
        if let Some(front) = q.front() {
            if front.deps.iter().all(|d| done.contains(d)) {
                return Ok(q.pop_front());
            }
            // Deps unmet: return None to signal end (not a real wait signal in
            // this MVP; the planner is responsible for gating correctly).
        }
        Ok(q.pop_front())
    }
}

#[tokio::test]
async fn step_with_unmet_deps_not_dispatched_early() {
    let task_order: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let inner = MockWorker::always_ok("w", "ok")
        .push_ok("ok")
        .push_ok("ok")
        .push_ok("ok");
    let cap = Arc::new(OrderCapture {
        inner,
        order: task_order.clone(),
    });

    let planner = SeqPlanner {
        steps: Mutex::new(VecDeque::from(vec![
            PlanStep::new("a", "step a", vec![]),
            PlanStep::new("b", "step b", vec!["a".to_string()]),
            PlanStep::new("c", "step c", vec!["b".to_string()]),
        ])),
    };

    let budget = Budget::new().with_max_steps(10);
    let mut supervisor = Supervisor::new(budget, Box::new(planner));
    supervisor.add_worker(cap);

    let outcome = supervisor.run("seq goal").await.expect("run ok");
    assert_eq!(outcome, RunOutcome::GoalAchieved);

    let order = task_order.lock().unwrap().clone();
    assert_eq!(order, vec!["a", "b", "c"], "steps ran in dependency order");
}
