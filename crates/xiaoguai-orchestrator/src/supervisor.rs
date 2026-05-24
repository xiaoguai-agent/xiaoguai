//! `Supervisor` — the orchestration entry point.
//!
//! ## Run loop (with optional Challenger)
//!
//! ```text
//! loop {
//!   check budget → BudgetExhausted?
//!   planner.next_step(goal, history) → None  → GoalAchieved
//!                                    → Some(step) →
//!     if step.risk_level == High && challenger present:
//!       challenger.critique(step) →
//!         Accept          → dispatch as normal
//!         RequestRevision → re-ask planner with critique; loop ≤ MAX_REVISIONS
//!         Reject          → record skipped StepResult; continue
//!     else:
//!       dispatch directly
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
//! - Challenger memory / cross-run audit trail

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::budget::Budget;
use crate::challenger::{Challenger, Verdict};
use crate::error::OrchestratorError;
use crate::plan::{PlanStep, RiskLevel};
use crate::planner::Planner;
use crate::worker::{Task, Worker, WorkerResult};
use crate::worker_handle::WorkerPool;

/// Maximum number of revision loops for a single step before the supervisor
/// gives up and dispatches the (possibly still-risky) revised step anyway.
const MAX_REVISIONS: u32 = 3;

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
    /// Critique produced by the Challenger, if any.  `None` for steps that
    /// bypassed the challenger (`risk_level` < High or no challenger configured).
    pub critique_reasons: Option<Vec<String>>,
    /// Risk score from the Challenger, if any.
    pub critique_risk_score: Option<f64>,
}

impl StepResult {
    /// Build a plain (non-challenged) result.
    fn plain(step_id: String, description: String, success: bool, output: String) -> Self {
        Self {
            step_id,
            description,
            success,
            output,
            critique_reasons: None,
            critique_risk_score: None,
        }
    }

    /// Build a result that was rejected by the Challenger (skipped).
    fn rejected(step: &PlanStep, reasons: Vec<String>, risk_score: f64) -> Self {
        Self {
            step_id: step.id.clone(),
            description: step.description.clone(),
            success: false,
            output: format!(
                "Rejected by challenger (risk={risk_score:.2}): {}",
                reasons.join("; ")
            ),
            critique_reasons: Some(reasons),
            critique_risk_score: Some(risk_score),
        }
    }
}

/// Supervisor — owns the budget, planner, worker pool, and optional Challenger.
///
/// Workers are type-erased (`dyn Worker`) so the pool can hold heterogeneous
/// implementations.  Use `add_worker` to register workers before calling `run`.
pub struct Supervisor {
    budget: Budget,
    planner: Box<dyn Planner>,
    pool: WorkerPool,
    /// Optional challenger middleware.  Only invoked for `RiskLevel::High` steps.
    challenger: Option<Arc<dyn Challenger>>,
}

impl Supervisor {
    /// Build a supervisor with an empty worker pool and no challenger.
    pub fn new(budget: Budget, planner: Box<dyn Planner>) -> Self {
        Self {
            budget,
            planner,
            pool: WorkerPool::new(),
            challenger: None,
        }
    }

    /// Attach a challenger.  High-risk steps will be critiqued before dispatch.
    #[must_use]
    pub fn with_challenger(mut self, challenger: Arc<dyn Challenger>) -> Self {
        self.challenger = Some(challenger);
        self
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

            // Challenge high-risk steps before dispatching.
            let step_result = self
                .challenge_and_dispatch(goal, step, &mut history)
                .await?;
            if step_result.is_none() {
                // revision loop exhausted without settlement — should not
                // normally happen, but guard against an infinite loop.
                continue;
            }
            let step_result = step_result.unwrap();

            debug!(step_id = %step_result.step_id, success = step_result.success, "step done");
            history.push(step_result);
            steps_taken += 1;
        }
    }

    /// Route a step through the challenger (if configured and step is High risk)
    /// and return the final `StepResult`.
    ///
    /// Returns `None` only if `MAX_REVISIONS` is somehow exceeded without
    /// settling — treated as a safety skip in the caller.
    async fn challenge_and_dispatch(
        &self,
        goal: &str,
        mut step: PlanStep,
        history: &mut Vec<StepResult>,
    ) -> Result<Option<StepResult>, OrchestratorError> {
        // Non-high-risk steps or no challenger configured: dispatch directly.
        if step.risk_level != RiskLevel::High || self.challenger.is_none() {
            return Ok(Some(self.do_dispatch(&step, history).await?));
        }

        let challenger = self.challenger.as_ref().unwrap();

        for revision in 0..=MAX_REVISIONS {
            let critique = challenger.critique(&step).await?;

            match critique.verdict {
                Verdict::Accept => {
                    debug!(step_id = %step.id, "challenger accepted step");
                    return Ok(Some(self.do_dispatch(&step, history).await?));
                }
                Verdict::Reject => {
                    warn!(
                        step_id = %step.id,
                        risk = critique.risk_score,
                        "challenger rejected step — skipping"
                    );
                    return Ok(Some(StepResult::rejected(
                        &step,
                        critique.reasons,
                        critique.risk_score,
                    )));
                }
                Verdict::RequestRevision => {
                    if revision >= MAX_REVISIONS {
                        warn!(
                            step_id = %step.id,
                            "max revisions ({MAX_REVISIONS}) reached — dispatching as-is"
                        );
                        return Ok(Some(self.do_dispatch(&step, history).await?));
                    }
                    // Ask the planner for a revised step, passing the critique.
                    let critique_ctx = critique.to_context_string();
                    info!(
                        step_id = %step.id,
                        revision,
                        %critique_ctx,
                        "challenger requested revision — re-asking planner"
                    );

                    // Temporarily push a fake "revision request" into history so
                    // the planner sees the critique.  We remove it afterwards.
                    let temp_result = StepResult::plain(
                        format!("__revision_{}_for_{}", revision, step.id),
                        critique_ctx.clone(),
                        false,
                        critique_ctx,
                    );
                    history.push(temp_result);

                    let revised = self.planner.next_step(goal, history).await?;
                    // Remove the temporary marker.
                    history.pop();

                    match revised {
                        Some(s) => step = s,
                        None => {
                            // Planner gave up — treat as reject.
                            return Ok(Some(StepResult::rejected(
                                &step,
                                critique.reasons,
                                critique.risk_score,
                            )));
                        }
                    }
                }
            }
        }

        // Should be unreachable (loop exits via return), but satisfy the compiler.
        Ok(None)
    }

    /// Dispatch a step to the worker pool and wrap the result as a `StepResult`.
    async fn do_dispatch(
        &self,
        step: &PlanStep,
        history: &[StepResult],
    ) -> Result<StepResult, OrchestratorError> {
        let result = dispatch_step(&self.pool, step, history).await;
        match result {
            Ok(wr) => Ok(StepResult::plain(
                step.id.clone(),
                step.description.clone(),
                wr.success,
                wr.output,
            )),
            Err(OrchestratorError::WorkerFailed(msg)) => {
                warn!(step_id = %step.id, %msg, "worker failed; recording and continuing");
                Ok(StepResult::plain(
                    step.id.clone(),
                    step.description.clone(),
                    false,
                    msg,
                ))
            }
            Err(e) => Err(e),
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
