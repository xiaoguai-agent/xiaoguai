//! Human-on-the-Loop (HOTL) boundary policy — v1.2.3.
//!
//! Institutional AI deployments require "set the budgets, let the agent run,
//! escalate when budgets are breached". This module provides:
//!
//! * [`policy`] — CRUD types and the [`HotlPolicyStore`] trait (backed by PG
//!   in production; in-memory for tests).
//! * [`enforcer`] — the budget checker: window-bucketed counter + cost
//!   accumulator. Calls [`HotlEnforcer::check`] before each gated action.
//!
//! ## Verdict semantics
//!
//! * [`HotlVerdict::Allow`]   — under budget, proceed.
//! * [`HotlVerdict::Escalate`] — budget breached; notify `escalate_to` and
//!   allow the action (human reviews asynchronously). This is the default when
//!   `escalate_to` is configured.
//! * [`HotlVerdict::Deny`]    — budget breached and `escalate_to` is None, OR
//!   the PG backend is unreachable (fail-closed).
//!
//! ## Wiring
//!
//! The enforcer is wired into the LLM call path in this milestone. Email and
//! webhook action sites are follow-ups (tracked in docs/plans/).
//!
//! ## Fail-closed
//!
//! If the PG store is unreachable, [`HotlEnforcer::check`] returns
//! `Ok(HotlVerdict::Deny(...))` rather than propagating the error — the
//! system prefers denying a single call over allowing unbounded spend when
//! the budget ledger is unavailable.

pub mod audit;
pub mod decision;
pub mod enforcer;
pub mod policy;

pub use audit::{HotlAuditSink, InMemoryHotlAuditSink};
pub use decision::{
    HotlDecisionRecord, HotlDecisionStore, HotlDecisionStoreError, HotlDecisionVerdict,
    InMemoryHotlDecisionStore,
};
pub use enforcer::{HotlEnforcer, HotlVerdictResult, StaticHotlEnforcer};
pub use policy::{
    CreateHotlPolicyRequest, HotlPolicy, HotlPolicyStore, HotlPolicyStoreError,
    InMemoryHotlPolicyStore,
};
