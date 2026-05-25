//! `xiaoguai-tasks` — durable Kanban task board + auto-dispatcher (ADR-0019).
//!
//! This crate currently ships **two layers** that landed on separate branches
//! (`feat/kanban-backend-tasks` + `feat/kanban-auto-dispatcher`) and were
//! integrated together in the v1.4 merge wave:
//!
//! ## 1. Persistence layer (`types` / `traits` / `mem` / `pg`)
//!
//! A first-class board where agents pick up and finish tasks, with every
//! column transition recorded as an outcome event for the telemetry pipeline.
//!
//! ```text
//! TaskBoardRepository (trait)
//!   ├── InMemoryTaskBoardRepository  (unit tests, no DB)
//!   └── PgTaskBoardRepository        (Postgres, migration 0018)
//! OutcomeAttribution (trait)          — wires column transitions into telemetry
//! ```
//!
//! ## 2. Dispatcher layer (`card` / `dispatcher` / `executor` / `metrics` / `store`)
//!
//! A worker pool that drives cards through their lifecycle:
//!
//! ```text
//! READY  →  (claim via SKIP LOCKED)  →  RUNNING  →  DONE / BLOCKED (retries N times)
//! ```
//!
//! * [`KanbanCard`] / [`CardStore`] — the dispatcher's unit of work + its store seam.
//! * [`WorkerPool`] — configurable pool size, poll interval, retry, timeout, graceful shutdown.
//! * [`TaskExecutor`] — injectable async trait producing an [`Outcome`].
//! * [`PoolMetrics`] — Prometheus counters emitted by the pool.
//!
//! ## Reconciliation status (follow-up)
//!
//! The two layers use parallel type models — persistence (`Task`/`Column`/
//! `TaskBoardRepository`) and dispatcher (`KanbanCard`/`CardColumn`/`CardStore`).
//! They are independent and both compile; unifying them behind one type model
//! (so the `WorkerPool` claims directly from `PgTaskBoardRepository`) is tracked
//! as a follow-up. Until then a thin bridge maps between the two.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::needless_pass_by_value
)]

// Persistence layer.
pub mod mem;
pub mod pg;
pub mod traits;
pub mod types;

// Dispatcher layer.
pub mod card;
pub mod dispatcher;
pub mod executor;
pub mod metrics;
pub mod store;

// Public re-exports — persistence layer.
pub use mem::InMemoryTaskBoardRepository;
pub use pg::PgTaskBoardRepository;
pub use traits::{OutcomeAttribution, TaskBoardRepository};
pub use types::{Board, Column, CreateBoardRequest, CreateTaskRequest, Task, TaskStateLogEntry};

// Public re-exports — dispatcher layer.
pub use card::{Attribution, CardColumn, CardId, KanbanCard, Outcome};
pub use dispatcher::{PoolConfig, WorkerPool};
pub use executor::{ExecutorError, MockExecutor, TaskExecutor};
pub use metrics::PoolMetrics;
pub use store::{CardStore, InMemoryCardStore, StoreError};
