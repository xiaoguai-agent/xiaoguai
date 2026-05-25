//! Xiaoguai orchestrator — supervisor pattern.
//!
//! A `Supervisor` holds a `Planner` (which decomposes a high-level goal into
//! `PlanStep`s), a pool of `Worker`s, and a `Budget` (step/token/time cap).
//!
//! Run loop:
//! 1. Ask planner for the next step (passing accumulated history).
//! 2. If planner returns `None` → `RunOutcome::GoalAchieved`.
//! 3. Verify the step's dependency ids are already in the success history.
//!    If not, the step is skipped this round (planner will re-emit later).
//! 4. Pick a worker from the pool (round-robin) and call `worker.execute`.
//! 5. Record the `StepResult` (success or failure).
//! 6. Increment step counter; if counter hits `Budget::max_steps` →
//!    `RunOutcome::BudgetExhausted`.
//! 7. Go to 1.
//!
//! Design goals:
//! - No LLM dependency in this crate.  Callers provide a `Planner` impl;
//!   a `MockPlanner` is used in tests.  An LLM-backed planner lives in
//!   xiaoguai-core or the binary layer so this crate stays light.
//! - Serial worker dispatch in v1.1.5b.  Parallel dispatch (multiple steps
//!   per round) is deferred to v1.2 once the API surface settles.

#![forbid(unsafe_code)]

pub mod budget;
pub mod challenger;
pub mod error;
pub mod plan;
pub mod planner;
pub mod registry;
pub mod supervisor;
pub mod worker;
pub mod worker_handle;

pub use budget::Budget;
pub use challenger::{Challenger, Critique, MockChallenger, Verdict};
pub use error::OrchestratorError;
pub use plan::{Plan, PlanStep, RiskLevel, StepStatus};
pub use planner::Planner;
pub use registry::conflict::{AgentConflict, ConflictArbitrator, ConflictPolicy, ResourceKey};
pub use registry::router::{CapabilityRouter, Dispatch, Intent};
pub use registry::store::{InMemoryStore, PgStore, RegistryStore};
pub use registry::{
    Agent, AgentRef, AgentRegistry, AgentSpec, Capability, ResultShape, TaskShape, TenantScope,
};
pub use supervisor::{RunOutcome, RunReport, StepResult, Supervisor};
pub use worker::{Task, Worker, WorkerResult};
