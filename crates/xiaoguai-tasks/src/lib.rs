//! `xiaoguai-tasks` — agent-authored skill proposals (HotL-gated).
//!
//! * [`skill_author`] — proposal types + the `SkillProposalRepository` /
//!   `TenantSettingsReader` / `SkillAuthorGate` / `SkillAuditSink` trait seams
//!   (Tier-2 D.1, DEC-023.3): an agent drafts a skill, `HotL` review approves.
//! * [`skill_author_sqlite`] — `SQLite` implementations of those traits.
//!
//! ## Historical note (node-scope cleanup, 2026-06-10)
//!
//! This crate originally also shipped the ADR-0019 Kanban task board +
//! auto-dispatcher (`types`/`traits`/`mem`/`sqlite` persistence and
//! `card`/`dispatcher`/`executor`/`metrics`/`store` worker pool). That was a
//! fleet-workforce surface — a board dispatching work to a pool of agents —
//! which contradicts DEC-033: xiaoguai is one self-contained governed agent
//! node, not a control plane over an agent fleet. The modules were never wired
//! into the runtime (the admin-ui pane ran on a mock fallback) and were
//! removed; `git log` has them if a *node-local* work queue is ever wanted.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::needless_pass_by_value
)]

// Tier-2 D.1 — agent-authored skill proposals (HotL-gated, admin-approved).
pub mod skill_author;
// Sprint-8 S8-7 (DEC-023.3) — SQLite impls of the skill_author traits.
pub mod skill_author_sqlite;
