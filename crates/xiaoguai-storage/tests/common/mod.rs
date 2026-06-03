//! Shared `SQLite` fixture for repository integration tests (DEC-033 single-user).
//!
//! Each test opens an isolated temp-file `SQLite` database and applies the
//! crate's migrations. The `TempDir` guard returned alongside the pool must be
//! kept alive for the duration of the test — when it drops, the database file
//! (and WAL sidecars) are removed.

#![allow(dead_code)]

use sqlx::SqlitePool;
use tempfile::TempDir;
use xiaoguai_storage::db;

/// Open a temp `SQLite` database, connect a pool, and run all migrations.
///
/// Returns the pool and the owning `TempDir` (keep it in scope). No Docker, no
/// network — these tests run anywhere.
///
/// # Panics
///
/// Panics if the temp dir cannot be created or migrations fail.
pub async fn test_setup() -> (SqlitePool, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("test.db");
    let pool = db::connect(path.to_str().expect("utf8 path"), 5)
        .await
        .expect("connect");
    db::migrate(&pool).await.expect("migrate");
    (pool, dir)
}
