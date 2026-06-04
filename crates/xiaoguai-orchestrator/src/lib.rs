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
/// v1.6+ orchestration patterns (DEC-021, sprint-9). The patterns
/// module hosts the runtime-facing event types (`OrchEvent`,
/// `TriangleStopReason`) so they don't pollute the v1.4 `supervisor`
/// flow. See `patterns/triangle.rs` for the planner/worker/critic
/// runner.
pub mod patterns;
pub mod plan;
pub mod planner;
pub mod registry;
pub mod supervisor;
/// v1.6+ planner/worker/critic triangle (DEC-021, sprint-9). The
/// types here intentionally do NOT collide with the v1.4 `budget`,
/// `plan`, or `challenger::Verdict` — they live under
/// `xiaoguai_orchestrator::triangle::*`. See `lld-orchestrator.md`
/// §4.4–4.7.
pub mod triangle;
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
pub use registry::{Agent, AgentRef, AgentRegistry, AgentSpec, Capability, ResultShape, TaskShape};
pub use supervisor::{RunOutcome, RunReport, StepResult, Supervisor};
pub use worker::{Task, Worker, WorkerResult};
