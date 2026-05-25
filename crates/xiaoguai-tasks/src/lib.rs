//! `xiaoguai-tasks` — Kanban dispatcher + worker pool (ADR-0019).
//!
//! This crate provides the auto-dispatcher that drives the Kanban board:
//!
//! ```text
//! READY  →  (claim via SKIP LOCKED)  →  RUNNING
//!                                          │
//!                          ┌──────────────┤
//!                          │              │
//!                        DONE          BLOCKED
//!                                   (retries N times)
//! ```
//!
//! # Key types
//!
//! * [`KanbanCard`] — a unit of work tracked through column transitions.
//! * [`TaskExecutor`] — injectable async trait: given a card, produce an
//!   [`Outcome`] or an [`ExecutorError`].
//! * [`WorkerPool`] — the dispatcher: configurable pool size + poll interval,
//!   concurrent-safe claim (SKIP LOCKED semantics via in-memory lock), retry,
//!   timeout, and graceful-shutdown on SIGTERM.
//! * [`PoolMetrics`] — Prometheus counters emitted by the pool.
//!
//! # Why not wire into `AppState`
//!
//! The dispatcher intentionally has no `xiaoguai-core` dependency. It bridges
//! in during the integration phase together with `feat/kanban-backend-tasks`
//! where the PG-backed `CardStore` lands. The `InMemoryCardStore` here is the
//! production-shaped seam: swap it for a PG impl and the pool doesn't change.
//!
//! # Environment variables
//!
//! | Variable | Default | Description |
//! |---|---|---|
//! | `KANBAN_POOL_SIZE` | `10` | Worker concurrency |
//! | `KANBAN_POLL_INTERVAL_MS` | `5000` | Poll cadence in milliseconds |
//! | `KANBAN_TASK_TIMEOUT_SECS` | `300` | Per-task timeout in seconds |
//! | `KANBAN_MAX_RETRIES` | `3` | Max attempts before BLOCKED |

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod card;
pub mod dispatcher;
pub mod executor;
pub mod metrics;
pub mod store;

pub use card::{Attribution, CardColumn, CardId, KanbanCard, Outcome};
pub use dispatcher::{PoolConfig, WorkerPool};
pub use executor::{ExecutorError, MockExecutor, TaskExecutor};
pub use metrics::PoolMetrics;
pub use store::{CardStore, InMemoryCardStore, StoreError};
