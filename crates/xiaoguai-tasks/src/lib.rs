//! `xiaoguai-tasks` — durable Kanban task board for multi-agent work queues.
//!
//! Implements ADR-0019: a first-class board where agents autonomously pick up
//! and finish tasks, with every column transition recorded as an outcome event
//! for the existing telemetry pipeline.
//!
//! ## Architecture
//!
//! ```text
//! TaskBoardRepository (trait)
//!   ├── InMemoryTaskBoardRepository  (unit tests, no DB)
//!   └── PgTaskBoardRepository        (Postgres, migration 0016)
//! OutcomeAttribution (trait)          — wires column transitions into telemetry
//! ```
//!
//! ## Modules
//!
//! | Module   | Contents |
//! |----------|----------|
//! | [`types`]  | `Board`, `Task`, `TaskStateLogEntry`, `Column` domain types |
//! | [`traits`] | `TaskBoardRepository` + `OutcomeAttribution` traits |
//! | [`mem`]    | `InMemoryTaskBoardRepository` — for tests |
//! | [`pg`]     | `PgTaskBoardRepository` — Postgres implementation |

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod mem;
pub mod pg;
pub mod traits;
pub mod types;

// Public re-exports — the full public API surface.
pub use mem::InMemoryTaskBoardRepository;
pub use pg::PgTaskBoardRepository;
pub use traits::{OutcomeAttribution, TaskBoardRepository};
pub use types::{Board, Column, CreateBoardRequest, CreateTaskRequest, Task, TaskStateLogEntry};
