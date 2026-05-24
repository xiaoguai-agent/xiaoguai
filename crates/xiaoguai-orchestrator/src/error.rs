//! Unified error type for the orchestrator crate.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum OrchestratorError {
    /// The planner failed to produce a plan step (non-transient).
    #[error("planner error: {0}")]
    Planner(String),

    /// A worker returned a failure result or panicked.
    #[error("worker failed: {0}")]
    WorkerFailed(String),

    /// Budget was exhausted (step cap, token cap, or wall-time cap).
    #[error("budget exhausted: {0}")]
    BudgetExhausted(String),

    /// An unexpected internal error.
    #[error("internal orchestrator error: {0}")]
    Internal(String),
}
