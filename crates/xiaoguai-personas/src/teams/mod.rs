//! Expert teams (T3) — named persona compositions with a designated lead.
//!
//! See `docs/plans/2026-06-10-expert-center.md`. Module layout mirrors the
//! persona modules one level up: [`model`], [`traits`], [`memory`], [`sqlite`].

pub mod memory;
pub mod model;
pub mod sqlite;
pub mod traits;

pub use memory::InMemoryTeamRepository;
pub use model::{CreateTeamRequest, SessionTeam, Team, UpdateTeamRequest};
pub use sqlite::SqliteTeamRepository;
pub use traits::TeamRepository;
