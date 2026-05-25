//! Unit tests for [`ReadWritePool`].
//!
//! No real Postgres is needed: `ReadWritePool` routes based on the vector
//! of replicas and an atomic counter. We verify the routing logic using
//! only the `&PgPool` pointer identity via raw pointer comparison.
//!
//! `connect_lazy` requires a Tokio runtime even for pool creation (sqlx
//! spawns an internal background task), so all tests are `#[tokio::test]`.
//!
//! Integration tests against a real PG replica topology would require a
//! testcontainer setup with logical replication enabled — that is deferred
//! per the v1.1.4.1 plan and would carry an `#[ignore]` marker.

#![cfg(test)]

use sqlx::postgres::PgPoolOptions;
use xiaoguai_storage::read_write_pool::ReadWritePool;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a dummy `PgPool` with a lazy connection. Never actually connects;
/// the connection is opened only on first query use.
fn dummy_pool(url: &str) -> sqlx::PgPool {
    PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy(url)
        .expect("connect_lazy should not fail for a dummy url")
}

// ---------------------------------------------------------------------------
// Tests — no-replica path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_replicas_reader_returns_primary() {
    let rwp = ReadWritePool::new(dummy_pool("postgres://invalid:5432/t_primary"), vec![]);

    // With no replicas, reader() and writer() must point at the same pool object.
    let writer_ptr = rwp.writer() as *const _;
    let reader_ptr = rwp.reader() as *const _;
    assert_eq!(
        writer_ptr, reader_ptr,
        "with no replicas, reader() must return the same pool as writer()"
    );
}

#[tokio::test]
async fn no_replicas_writer_is_primary() {
    let rwp = ReadWritePool::new(dummy_pool("postgres://invalid:5432/t_primary2"), vec![]);
    // Both writer() and reader() must return the same reference (primary).
    let writer_ptr = rwp.writer() as *const _;
    let reader_ptr = rwp.reader() as *const _;
    assert_eq!(
        writer_ptr, reader_ptr,
        "writer() must equal reader() when replicas is empty"
    );
}

// ---------------------------------------------------------------------------
// Tests — round-robin with replicas
// ---------------------------------------------------------------------------

/// With N replicas, `reader()` must cycle through them in order.
#[tokio::test]
async fn with_replicas_reader_round_robins() {
    let rwp = ReadWritePool::new(
        dummy_pool("postgres://invalid:5432/p_rr"),
        vec![
            dummy_pool("postgres://invalid:5432/r0"),
            dummy_pool("postgres://invalid:5432/r1"),
            dummy_pool("postgres://invalid:5432/r2"),
        ],
    );

    // Call reader() 6 times.
    let seq: Vec<*const sqlx::PgPool> = (0..6).map(|_| std::ptr::from_ref(rwp.reader())).collect();

    // Period must equal replica count (3).
    assert_eq!(seq[0], seq[3], "reader cycle should have period 3");
    assert_eq!(seq[1], seq[4]);
    assert_eq!(seq[2], seq[5]);

    // All three replicas must appear in the first lap.
    let unique: std::collections::HashSet<*const sqlx::PgPool> = seq[..3].iter().copied().collect();
    assert_eq!(unique.len(), 3, "all 3 replicas must appear in first lap");
}

#[tokio::test]
async fn writer_always_returns_primary_with_replicas() {
    let rwp = ReadWritePool::new(
        dummy_pool("postgres://invalid:5432/p_w"),
        vec![dummy_pool("postgres://invalid:5432/r_w")],
    );

    // With 1 replica, reader() returns the replica. writer() must differ.
    let writer_ptr = rwp.writer() as *const _;
    let reader_ptr = rwp.reader() as *const _;
    assert_ne!(writer_ptr, reader_ptr, "writer() must not return a replica");
}

// ---------------------------------------------------------------------------
// Tests — Clone semantics
// ---------------------------------------------------------------------------

/// Cloning a `ReadWritePool` shares the same atomic counter — calls on the
/// clone advance the counter seen by the original.
#[tokio::test]
async fn clone_shares_rr_counter() {
    let rwp1 = ReadWritePool::new(
        dummy_pool("postgres://invalid:5432/p_clone"),
        vec![
            dummy_pool("postgres://invalid:5432/rc0"),
            dummy_pool("postgres://invalid:5432/rc1"),
        ],
    );
    let rwp2 = rwp1.clone();

    let ptr_a = rwp1.reader() as *const _; // counter 0 → idx 0
    let ptr_b = rwp2.reader() as *const _; // counter 1 → idx 1
    let ptr_c = rwp1.reader() as *const _; // counter 2 → idx 0

    // ptr_a and ptr_c should hit the same replica (idx 0).
    assert_eq!(ptr_a, ptr_c, "every 2nd call returns the same replica");
    // ptr_b should hit the other replica (idx 1).
    assert_ne!(ptr_a, ptr_b, "consecutive calls alternate replicas");
}

// ---------------------------------------------------------------------------
// Integration test stub (requires real PG + logical replication)
// ---------------------------------------------------------------------------

/// Full round-trip: connect to primary + replica, verify both are reachable.
///
/// Skipped by default — requires a running PG primary + at least one replica
/// configured via `DATABASE_URL` + `DATABASE_REPLICA_URLS`.
#[tokio::test]
#[ignore = "requires PG primary+replica; set DATABASE_URL + DATABASE_REPLICA_URLS"]
async fn integration_read_routes_to_replica() {
    use xiaoguai_storage::db;

    let primary_url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let replica_urls: Vec<String> = std::env::var("DATABASE_REPLICA_URLS")
        .expect("DATABASE_REPLICA_URLS")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let primary = db::connect(&primary_url, 5).await.unwrap();
    let mut replicas = Vec::new();
    for url in &replica_urls {
        replicas.push(db::connect(url, 5).await.unwrap());
    }
    let rwp = ReadWritePool::new(primary, replicas);

    let _: (i32,) = sqlx::query_as("SELECT 1")
        .fetch_one(rwp.writer())
        .await
        .expect("writer select 1");
    let _: (i32,) = sqlx::query_as("SELECT 1")
        .fetch_one(rwp.reader())
        .await
        .expect("reader select 1");
}
