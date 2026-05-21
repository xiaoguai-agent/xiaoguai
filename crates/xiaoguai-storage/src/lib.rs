//! Storage layer — Postgres (via sqlx) + Valkey/Redis cache.
//!
//! All business crates depend on this for persistence. No SQL leaks past the
//! repository boundary. Multi-tenant queries are enforced here with mandatory
//! `tenant_id` filters + Postgres RLS as a defense-in-depth layer.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]

pub mod cache;
pub mod db;
pub mod migrations;
pub mod repositories;

pub use db::{connect, migrate};
