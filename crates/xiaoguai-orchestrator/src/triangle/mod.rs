//! Planner / Worker / Critic triangle — v1.6+ (DEC-021, sprint-9).
//!
//! Implements the heterogeneous multi-agent topology committed in
//! `xiaoguai-agent-design/docs/hld.md` DEC-021 and detailed in
//! `lld/lld-orchestrator.md` §4.4–4.7. This module ships in three
//! phases:
//!
//! - **S9-1 (this file + siblings)**: types + traits scaffolding, no
//!   behaviour. The Phase B sub-agents and Phase C/D work fill in the
//!   actual agents and pattern wiring.
//! - **S9-2 / S9-3 / S9-4**: `PlannerAgent` / `WorkerAgent` /
//!   `CriticAgent` implementations.
//! - **S9-5 / S9-6**: pattern wiring in `patterns/triangle.rs` plus
//!   integration tests.
//!
//! Quarantine invariant from §4.5 — each Worker writes to its own
//! `Scratchpad` keyed by `task_id`; no Worker can read another's
//! drafts. Critic reads scratchpads but cannot write. Only on
//! `Verdict::Approve` does a scratchpad's contents migrate into
//! session memory (visible to the next round's `MemorySnapshot`).

pub mod budget;
pub mod memory_view;
pub mod plan;
pub mod planner_agent;
pub mod roles;
pub mod scratchpad;
pub mod verdict;

pub use budget::{BudgetError, TriangleBudget};
pub use memory_view::{MemoryFact, MemorySnapshot, MemoryView};
pub use plan::{AcceptanceCriteria, Plan, Task, TaskId};
pub use planner_agent::{PlannerAgent, PlannerError};
pub use roles::Role;
pub use scratchpad::{ScratchEntry, Scratchpad, ScratchpadError};
pub use verdict::{Verdict, VerdictKind};
