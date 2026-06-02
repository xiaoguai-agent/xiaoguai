//! Phase-1 migration smoke (DEC-033 single-user SQLite pivot).
//!
//! Intentionally empty — all logic lives in `tests/sqlite_migrations_smoke.rs`.
//! This crate is the independently-verifiable Phase-1 gate: it proves the ported
//! SQLite migrations apply to a fresh database before the (Phase-2) repository
//! retype makes `xiaoguai-storage` compile again.
