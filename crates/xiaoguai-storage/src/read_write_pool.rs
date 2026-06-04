//! Single-writer pool handle (DEC-033 single-user pivot).
//!
//! `SQLite` is one file with one writer — there are no read replicas to route to.
//! [`ReadWritePool`] is now a thin wrapper over a single [`SqlitePool`] that
//! keeps the `reader()` / `writer()` shape so call sites in the repository and
//! bridge layers don't have to change. Both accessors return the same pool.
//!
//! The replica-routing machinery (`DATABASE_REPLICA_URLS`, round-robin) that
//! existed for the former Postgres deployment has been removed.

use sqlx::sqlite::SqlitePool;

/// A handle to the single-user `SQLite` pool.
///
/// `reader()` and `writer()` both return the one underlying pool — the
/// distinction is retained only so existing call sites keep compiling.
#[derive(Clone)]
pub struct ReadWritePool {
    pool: SqlitePool,
}

impl ReadWritePool {
    /// Wrap a single [`SqlitePool`].
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Returns the underlying pool. Use for all writes and explicit transactions.
    #[must_use]
    #[inline]
    pub fn writer(&self) -> &SqlitePool {
        &self.pool
    }

    /// Returns the underlying pool. (No replicas under `SQLite` — same as
    /// [`writer()`][Self::writer].)
    #[must_use]
    #[inline]
    pub fn reader(&self) -> &SqlitePool {
        &self.pool
    }

    /// Consume the wrapper and return the inner pool.
    #[must_use]
    pub fn into_inner(self) -> SqlitePool {
        self.pool
    }
}

impl From<SqlitePool> for ReadWritePool {
    fn from(pool: SqlitePool) -> Self {
        Self::new(pool)
    }
}
