//! Storage layer — embedded `SQLite` (via sqlx).
//!
//! All business crates depend on this for persistence. No SQL leaks past the
//! repository boundary. Single-user deployment (DEC-033): one `SQLite` file, one
//! implicit owner — no tenants, no row-level security.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]

pub mod db;
pub mod migrations;
pub mod read_write_pool;
pub mod repositories;

pub use db::{connect, migrate};
pub use read_write_pool::ReadWritePool;
