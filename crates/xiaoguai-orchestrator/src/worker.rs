//! `Worker` trait and associated types.
//!
//! A worker is an opaque unit of execution: given a `Task`, it returns a
//! `WorkerResult`.  In production the impl wraps a `xiaoguai-agent`
//! `ReactAgent`; in tests a `MockWorker` is used.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::OrchestratorError;

/// The input handed to a worker for a single plan step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// The step id from `PlanStep::id`.
    pub step_id: String,
    /// Human-readable description of what to do — passed verbatim to the
    /// underlying agent as the user message.
    pub description: String,
    /// Optional structured context assembled by the supervisor from prior
    /// step results.  Workers may ignore this.
    pub context: Vec<String>,
}

/// The outcome of one worker execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerResult {
    /// Natural-language summary of what the worker produced.
    pub output: String,
    /// `true` if the worker considers the subtask done, `false` on failure.
    pub success: bool,
}

/// Execute a single sub-task and return a result.
///
/// Workers are expected to be stateless between calls — any per-run state
/// lives in the `Task` or in the worker's backing agent run.
#[async_trait]
pub trait Worker: Send + Sync {
    async fn execute(&self, task: Task) -> Result<WorkerResult, OrchestratorError>;
}
