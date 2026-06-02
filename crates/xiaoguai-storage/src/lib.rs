//! Storage layer — embedded SQLite (via sqlx) + Valkey/Redis cache.
//!
//! All business crates depend on this for persistence. No SQL leaks past the
//! repository boundary. Single-user deployment (DEC-033): one SQLite file, one
//! implicit owner — no tenants, no row-level security.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]

/// The single implicit owner's tenant id (DEC-033). The `tenant_id` columns are
/// gone from the SQLite schema, but domain types in `xiaoguai-types` still carry
/// a `TenantId`; repositories synthesise this constant on read and ignore the
/// vestigial `tenant` parameter on write. A later cleanup may drop the field
/// from the domain types entirely.
pub const OWNER_TENANT_ID: &str = "ten_local_owner";

pub mod cache;
pub mod db;
pub mod migrations;
pub mod read_write_pool;
pub mod repositories;

pub use db::{connect, migrate};
pub use read_write_pool::ReadWritePool;
