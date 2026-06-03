//! Migration smoke test against an embedded `SQLite` database (DEC-033).
//!
//! No Docker — `common::test_setup` opens a temp database and applies every
//! migration. Asserts the suite applies clean and key tables exist.

mod common;

use common::test_setup;

/// Whether a table exists in the `SQLite` catalog.
async fn table_exists(pool: &sqlx::SqlitePool, name: &str) -> bool {
    let (count,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?")
            .bind(name)
            .fetch_one(pool)
            .await
            .expect("query sqlite_master");
    count > 0
}

#[tokio::test]
async fn migrations_apply_clean() {
    // test_setup() panics if any migration fails to apply.
    let (pool, _guard) = test_setup().await;

    // Single-user schema: no `tenants` table.
    assert!(
        !table_exists(&pool, "tenants").await,
        "tenants table should be gone under the single-user pivot"
    );

    // Core tables created by 0001 + later migrations.
    assert!(table_exists(&pool, "users").await, "users table should exist");
    assert!(
        table_exists(&pool, "sessions").await,
        "sessions table should exist"
    );
    assert!(
        table_exists(&pool, "audit_log").await,
        "audit_log table should exist"
    );

    // The `users` table is empty on a fresh database.
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM users")
        .fetch_one(&pool)
        .await
        .expect("count users");
    assert_eq!(count, 0);
}
