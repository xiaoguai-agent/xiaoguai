//! `Supervisor` — the orchestration entry point.
//!
//! ## Run loop
//!
//! ```text
//! loop {
//!   check budget → BudgetExhausted?
//!   planner.next_step(goal, history) → None  → GoalAchieved
//!                                    → Some(step) → dispatch
//!   worker_pool.next() → execute(task)
//!   record StepResult in history
//!   steps_taken += 1
//! }
//! ```
//!
//! ## Deferred to v1.2
//! - Parallel worker dispatch (multiple steps per round)
//! - Dynamic re-planning (planner sees worker output mid-run)
//! - Cancel token propagation to in-flight workers
//! - Token-usage bubbling from worker runs

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::budget::Budget;
use crate::error::OrchestratorError;
use crate::plan::PlanStep;
use crate::planner::Planner;
use crate::worker::{Task, Worker, WorkerResult};
use crate::worker_handle::WorkerPool;

/// The terminal outcome of a supervisor run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    /// The planner returned `None` — all planned steps completed.
    GoalAchieved,
    /// A budget limit (steps / tokens / wall time) was reached.
    BudgetExhausted,
}

/// Record of one dispatched step and its outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    /// ID from the dispatched `PlanStep`.
    pub step_id: String,
    /// Human-readable description (copied from the `PlanStep`).
    pub description: String,
    /// `true` if the worker reported success.
    pub success: bool,
    /// The worker's output text (or error message on failure).
    pub output: String,
}

/// Supervisor — owns the budget, planner, and worker pool.
///
/// Workers are type-erased (`dyn Worker`) so the pool can hold heterogeneous
/// implementations.  Use `add_worker` to register workers before calling `run`.
pub struct Supervisor {
    budget: Budget,
    planner: Box<dyn Planner>,
    pool: WorkerPool,
}

impl Supervisor {
    /// Build a supervisor with an empty worker pool.
    pub fn new(budget: Budget, planner: Box<dyn Planner>) -> Self {
        Self {
            budget,
            planner,
            pool: WorkerPool::new(),
        }
    }

    /// Add a worker to the round-robin pool.
    pub fn add_worker(&mut self, w: Arc<dyn Worker>) {
        self.pool.add(w);
    }

    /// Run to completion or budget exhaustion.
    pub async fn run(&mut self, goal: &str) -> Result<RunOutcome, OrchestratorError> {
        let report = self.run_detailed(goal).await?;
        Ok(report.outcome)
    }

    /// Like `run` but also returns the full step history.
    pub async fn run_detailed(&mut self, goal: &str) -> Result<RunReport, OrchestratorError> {
        self.budget.start();
        let mut history: Vec<StepResult> = Vec::new();
        let mut steps_taken: u32 = 0;

        loop {
            // Budget check before each new step.
            if let Some(reason) = self.budget.check(steps_taken) {
                info!(goal, steps_taken, %reason, "budget exhausted");
                return Ok(RunReport {
                    outcome: RunOutcome::BudgetExhausted,
                    history,
                });
            }

            // Ask planner for next step.
            let step = match self.planner.next_step(goal, &history).await {
                Ok(Some(s)) => s,
                Ok(None) => {
                    info!(goal, steps_taken, "planner returned None → GoalAchieved");
                    return Ok(RunReport {
                        outcome: RunOutcome::GoalAchieved,
                        history,
                    });
                }
                Err(e) => {
                    warn!(goal, %e, "planner error");
                    return Err(e);
                }
            };

            let result = dispatch_step(&self.pool, &step, &history).await;
            let step_result = match result {
                Ok(wr) => StepResult {
                    step_id: step.id.clone(),
                    description: step.description.clone(),
                    success: wr.success,
                    output: wr.output,
                },
                Err(OrchestratorError::WorkerFailed(msg)) => {
                    warn!(step_id = %step.id, %msg, "worker failed; recording and continuing");
                    StepResult {
                        step_id: step.id.clone(),
                        description: step.description.clone(),
                        success: false,
                        output: msg,
                    }
                }
                Err(e) => return Err(e),
            };

            debug!(step_id = %step_result.step_id, success = step_result.success, "step done");
            history.push(step_result);
            steps_taken += 1;
        }
    }
}

async fn dispatch_step(
    pool: &WorkerPool,
    step: &PlanStep,
    history: &[StepResult],
) -> Result<WorkerResult, OrchestratorError> {
    let worker = pool
        .next()
        .ok_or_else(|| OrchestratorError::Internal("no workers in pool".to_string()))?;

    let context: Vec<String> = history
        .iter()
        .filter(|r| step.deps.contains(&r.step_id))
        .map(|r| format!("[{}] {}", r.step_id, r.output))
        .collect();

    let task = Task {
        step_id: step.id.clone(),
        description: step.description.clone(),
        context,
    };

    debug!(step_id = %step.id, "dispatching task to worker");
    worker.execute(task).await
}

/// The full result of a supervised run, including history.
#[derive(Debug)]
pub struct RunReport {
    pub outcome: RunOutcome,
    pub history: Vec<StepResult>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{plan::PlanStep, OrchestratorError, Task, WorkerResult};
    use async_trait::async_trait;

    struct OkWorker;

    #[async_trait]
    impl Worker for OkWorker {
        async fn execute(&self, task: Task) -> Result<WorkerResult, OrchestratorError> {
            Ok(WorkerResult {
                output: format!("done: {}", task.step_id),
                success: true,
            })
        }
    }

    struct OncePlanner(std::sync::Mutex<bool>);

    #[async_trait]
    impl Planner for OncePlanner {
        async fn next_step(
            &self,
            _goal: &str,
            _history: &[StepResult],
        ) -> Result<Option<PlanStep>, OrchestratorError> {
            let mut fired = self.0.lock().unwrap();
            if *fired {
                return Ok(None);
            }
            *fired = true;
            Ok(Some(PlanStep::new("only", "only step", vec![])))
        }
    }

    #[tokio::test]
    async fn single_step_run_achieves_goal() {
        let budget = Budget::new().with_max_steps(10);
        let planner = OncePlanner(std::sync::Mutex::new(false));
        let mut sup = Supervisor::new(budget, Box::new(planner));
        sup.add_worker(Arc::new(OkWorker));

        let report = sup.run_detailed("test").await.unwrap();
        assert_eq!(report.outcome, RunOutcome::GoalAchieved);
        assert_eq!(report.history.len(), 1);
        assert!(report.history[0].success);
    }

    #[tokio::test]
    async fn no_workers_returns_error() {
        let budget = Budget::new().with_max_steps(10);
        let planner = OncePlanner(std::sync::Mutex::new(false));
        let mut sup = Supervisor::new(budget, Box::new(planner));
        // No workers added.
        let err = sup.run("test").await;
        assert!(err.is_err());
    }
}
