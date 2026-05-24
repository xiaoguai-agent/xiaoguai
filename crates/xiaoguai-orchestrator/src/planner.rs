//! `Planner` trait — the strategy interface for goal decomposition.
//!
//! Implementations range from a hardcoded `MockPlanner` (tests/examples) to
//! an LLM-backed planner living in `xiaoguai-core` (runtime dependency).

use async_trait::async_trait;

use crate::error::OrchestratorError;
use crate::plan::PlanStep;
use crate::supervisor::StepResult;

/// Decides what to do next given the current goal and run history.
///
/// The supervisor calls `next_step` once per loop iteration.  Returning
/// `Ok(None)` signals that the goal is achieved; returning `Ok(Some(step))`
/// means "do this next".
#[async_trait]
pub trait Planner: Send + Sync {
    /// Produce the next `PlanStep`, or `None` when the goal is complete.
    ///
    /// `goal` — the high-level goal string passed to `Supervisor::run`.
    /// `history` — all `StepResult`s accumulated so far in this run, in
    ///   chronological order.  The planner uses this to decide whether to
    ///   continue, retry, or declare done.
    async fn next_step(
        &self,
        goal: &str,
        history: &[StepResult],
    ) -> Result<Option<PlanStep>, OrchestratorError>;
}
