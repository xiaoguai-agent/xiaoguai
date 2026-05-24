//! v1.1.4.1 — Replica-aware read/write pool routing.
//!
//! [`ReadWritePool`] wraps a primary `PgPool` plus an optional list of
//! read-replica `PgPool`s and routes:
//!
//! - **writes + transactions** → always primary (via [`ReadWritePool::writer`])
//! - **reads** → round-robin across replicas (via [`ReadWritePool::reader`]);
//!   falls back to primary when no replicas are configured or available.
//!
//! ## Construction
//!
//! Populate replicas from the `DATABASE_REPLICA_URLS` env var (comma-separated
//! connection strings). If the var is unset or empty the pool falls back to
//! primary-only behaviour — fully backward-compatible with v1.1.4 deployments.
//!
//! ```ignore
//! let primary = db::connect(&settings.database.url, 10).await?;
//! let replicas = ReadWritePool::replicas_from_env(5).await?;
//! let rw_pool = ReadWritePool::new(primary, replicas);
//! ```
//!
//! ## Round-robin semantics
//!
//! The counter is an `Arc<AtomicUsize>` so that `Clone` shares the counter
//! across all handles — useful when `AppState` is cloned per request. The
//! counter advances with `Relaxed` ordering (no cross-thread happens-before
//! needed; each `fetch_add` is independent).
//!
//! ## Safety
//!
//! `PgPool` is internally `Arc`-wrapped by sqlx; `Clone` is cheap.
//! `ReadWritePool::Clone` clones each pool handle (cheap Arc bump) and shares
//! the atomic counter.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use sqlx::postgres::{PgPool, PgPoolOptions};
use tracing::warn;

/// The env var name for replica connection strings (comma-separated).
pub const DATABASE_REPLICA_URLS_ENV: &str = "DATABASE_REPLICA_URLS";

/// A read/write pool router.
///
/// - [`writer()`][ReadWritePool::writer] — always returns the primary pool.
/// - [`reader()`][ReadWritePool::reader] — round-robins across replicas;
///   returns primary if no replicas are configured.
#[derive(Clone)]
pub struct ReadWritePool {
    primary: PgPool,
    replicas: Vec<PgPool>,
    /// Shared across clones so request-level clones stay in sync.
    rr: Arc<AtomicUsize>,
}

impl ReadWritePool {
    /// Create a new pool router.
    ///
    /// `replicas` may be empty — in that case [`reader()`][Self::reader]
    /// returns the primary (safe default, unchanged from pre-v1.1.4.1
    /// behaviour).
    #[must_use]
    pub fn new(primary: PgPool, replicas: Vec<PgPool>) -> Self {
        Self {
            primary,
            replicas,
            rr: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Parse `DATABASE_REPLICA_URLS` from the environment and connect to each
    /// URL using `max_connections` per pool.
    ///
    /// Returns an empty `Vec` (not an error) when the env var is absent or
    /// blank — this preserves backward compatibility.
    ///
    /// # Errors
    ///
    /// Returns an error only if a non-empty URL string fails to connect.
    pub async fn replicas_from_env(max_connections: u32) -> anyhow::Result<Vec<PgPool>> {
        let raw = match std::env::var(DATABASE_REPLICA_URLS_ENV) {
            Ok(v) if !v.trim().is_empty() => v,
            _ => return Ok(vec![]),
        };

        let mut pools = Vec::new();
        for url in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let pool = PgPoolOptions::new()
                .max_connections(max_connections)
                .connect(url)
                .await
                .map_err(|e| anyhow::anyhow!("replica connect {url}: {e}"))?;
            pools.push(pool);
        }
        if pools.is_empty() {
            warn!(
                env = DATABASE_REPLICA_URLS_ENV,
                "no replica URLs parsed — using primary for reads"
            );
        } else {
            tracing::info!(count = pools.len(), "read replicas configured");
        }
        Ok(pools)
    }

    /// Returns a reference to the **primary** pool.
    ///
    /// Use for all writes and explicit transactions.
    #[must_use]
    #[inline]
    pub fn writer(&self) -> &PgPool {
        &self.primary
    }

    /// Returns a reference to the **next replica** (round-robin).
    ///
    /// Falls back to primary when no replicas are configured.
    /// The counter is advanced atomically — safe to call concurrently.
    #[must_use]
    #[inline]
    pub fn reader(&self) -> &PgPool {
        if self.replicas.is_empty() {
            return &self.primary;
        }
        let idx = self.rr.fetch_add(1, Ordering::Relaxed) % self.replicas.len();
        &self.replicas[idx]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    fn lazy(url: &str) -> PgPool {
        PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(url)
            .expect("connect_lazy")
    }

    #[tokio::test]
    async fn no_replicas_reader_is_primary() {
        let p = lazy("postgres://invalid:5432/db");
        let rwp = ReadWritePool::new(p, vec![]);
        let writer = rwp.writer() as *const _;
        let reader = rwp.reader() as *const _;
        assert_eq!(
            writer, reader,
            "reader() must return primary when no replicas"
        );
    }

    #[tokio::test]
    async fn writer_always_returns_primary() {
        let p = lazy("postgres://invalid:5432/p");
        let r = lazy("postgres://invalid:5432/r");
        let rwp = ReadWritePool::new(p, vec![r]);
        // With 1 replica, reader() returns the replica.
        // writer() must NOT be the same reference.
        let writer = rwp.writer() as *const _;
        let reader = rwp.reader() as *const _;
        assert_ne!(writer, reader, "writer() must not return a replica");
    }

    #[tokio::test]
    async fn round_robin_cycles_through_all_replicas() {
        let p = lazy("postgres://invalid:5432/p");
        let r0 = lazy("postgres://invalid:5432/r0");
        let r1 = lazy("postgres://invalid:5432/r1");
        let r2 = lazy("postgres://invalid:5432/r2");
        let rwp = ReadWritePool::new(p, vec![r0, r1, r2]);

        let ptrs: Vec<*const PgPool> = (0..6).map(|_| rwp.reader() as *const _).collect();

        // Period must be 3.
        assert_eq!(ptrs[0], ptrs[3]);
        assert_eq!(ptrs[1], ptrs[4]);
        assert_eq!(ptrs[2], ptrs[5]);

        // All three replicas covered in first lap.
        let unique: std::collections::HashSet<_> = ptrs[..3].iter().cloned().collect();
        assert_eq!(unique.len(), 3);
    }

    #[tokio::test]
    async fn clone_shares_counter() {
        let p = lazy("postgres://invalid:5432/p_clone");
        let r0 = lazy("postgres://invalid:5432/rc0");
        let r1 = lazy("postgres://invalid:5432/rc1");
        let rwp1 = ReadWritePool::new(p, vec![r0, r1]);
        let rwp2 = rwp1.clone();

        let a = rwp1.reader() as *const _; // counter 0 → idx 0
        let b = rwp2.reader() as *const _; // counter 1 → idx 1
        let c = rwp1.reader() as *const _; // counter 2 → idx 0

        assert_eq!(a, c, "counter shared: every 2nd call returns same replica");
        assert_ne!(a, b, "consecutive calls alternate replicas");
    }
}
