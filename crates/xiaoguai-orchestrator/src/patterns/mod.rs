//! Orchestration patterns — v1.6+ (DEC-021, sprint-9).
//!
//! Each pattern wires together the agent primitives from the rest of
//! the crate into a structured loop. Patterns intentionally live in
//! their own module so the runtime-facing types (`OrchEvent`,
//! `StopReason`) can vary across patterns without polluting the v1.4
//! `supervisor` flow.
//!
//! Currently ships only the planner/worker/critic triangle. The other
//! three patterns referenced in `lld-orchestrator.md` §1
//! (Plan-then-execute / Researcher+Critic / Round-robin) remain in
//! their v1.4 incarnations inside `supervisor.rs` and `challenger.rs`
//! and will be ported into this module in a later sprint.

pub mod triangle;

pub use triangle::{OrchEvent, SessionId, TriangleRequest, TriangleRunner, TriangleStopReason};
