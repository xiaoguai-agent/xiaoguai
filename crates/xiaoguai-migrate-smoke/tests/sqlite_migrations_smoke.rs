//! Phase-1 verification gate (DEC-033): apply every ported migration to a fresh
//! SQLite file and assert the resulting schema is correct + single-user-shaped.
//!
//! The migrations are embedded from the sibling storage crate so this test does
//! not depend on `xiaoguai-storage` itself (still PgPool-typed until Phase 2).

use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::Row;

/// Tables that must exist after all migrations apply.
const EXPECTED_TABLES: &[&str] = &[
    "users",
    "sessions",
    "messages",
    "audit_log",
    "llm_providers",
    "token_usage",
    "mcp_servers",
    "scheduled_jobs",
    "hotl_policies",
    "memories",
    "recall_traces",
    "workspaces",
    "boards",
    "tasks",
    "hotl_escalations",
    "hotl_pending",
    "hotl_redaction_policies",
    "skill_proposals",
];

/// Tables that must NOT exist under the single-user pivot.
const FORBIDDEN_TABLES: &[&str] = &["tenants", "casbin_rule"];

#[tokio::test]
async fn migrations_apply_to_fresh_sqlite() {
    let tmp = tempfile::NamedTempFile::new().expect("create temp db file");
    let opts = SqliteConnectOptions::new()
        .filename(tmp.path())
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .expect("open sqlite pool");

    // The gate: every migration must apply with zero errors.
    sqlx::migrate!("../xiaoguai-storage/migrations")
        .run(&pool)
        .await
        .expect("all migrations apply cleanly to a fresh SQLite database");

    let rows = sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table'")
        .fetch_all(&pool)
        .await
        .expect("read sqlite_master");
    let tables: Vec<String> = rows.iter().map(|r| r.get::<String, _>("name")).collect();

    for expected in EXPECTED_TABLES {
        assert!(
            tables.iter().any(|t| t == expected),
            "expected table `{expected}` is missing; got {tables:?}"
        );
    }
    for forbidden in FORBIDDEN_TABLES {
        assert!(
            !tables.iter().any(|t| t == forbidden),
            "forbidden table `{forbidden}` must not exist under single-user (DEC-033)"
        );
    }

    // No `tenant_id` column survives anywhere.
    assert!(
        !column_exists(&pool, "sessions", "tenant_id").await,
        "sessions.tenant_id must be dropped under single-user"
    );

    // Seed sanity: 0020 promoted Ollama to the default (fallback_order = 1).
    let ollama_default: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM llm_providers WHERE id = 'ollama-local' AND fallback_order = 1",
    )
    .fetch_one(&pool)
    .await
    .expect("query ollama seed");
    assert_eq!(ollama_default, 1, "0020 ollama-default seed did not apply");

    // content_embedding must be a BLOB column (pgvector -> BLOB).
    assert!(
        column_is_blob(&pool, "memories", "content_embedding").await,
        "memories.content_embedding must be declared BLOB"
    );
}

/// Does `table.column` exist?
async fn column_exists(pool: &sqlx::SqlitePool, table: &str, column: &str) -> bool {
    let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(pool)
        .await
        .expect("pragma table_info");
    rows.iter().any(|r| r.get::<String, _>("name") == column)
}

/// Is `table.column` declared with BLOB type affinity?
async fn column_is_blob(pool: &sqlx::SqlitePool, table: &str, column: &str) -> bool {
    let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(pool)
        .await
        .expect("pragma table_info");
    rows.iter()
        .find(|r| r.get::<String, _>("name") == column)
        .is_some_and(|r| r.get::<String, _>("type").to_uppercase() == "BLOB")
}
