//! sprint-13 S13-3: read-only `HotlRedactionRepo::load_all` round-trip.
//!
//! Embedded `SQLite` (DEC-033). No Docker — each test opens a temp database via
//! `common::test_setup`. Single-owner deployment: every policy is owner-wide.
//! The repo sorts exact-scope rules before the `*` catch-all (S13-4 picks the
//! most specific match first).

mod common;

use common::test_setup;
use sqlx::SqlitePool;
use uuid::Uuid;
use xiaoguai_storage::repositories::hotl_redaction::{HotlRedactionRepo, PgHotlRedactionRepo};

/// Insert a redaction policy via raw SQL (the repo is read-only). `applies_to`
/// is a JSON-array TEXT column under `SQLite`.
async fn insert_policy(
    pool: &SqlitePool,
    scope: &str,
    jsonpath: &str,
    applies_to: &[&str],
) -> Uuid {
    let id = Uuid::new_v4();
    let applies = serde_json::to_string(applies_to).expect("serialize applies_to");
    sqlx::query(
        "INSERT INTO hotl_redaction_policies (id, scope, jsonpath, applies_to) \
         VALUES (?, ?, ?, ?)",
    )
    // Bind the UUID natively so sqlx stores it in the 16-byte form the repo's
    // `id: Uuid` decoder expects (a hyphenated TEXT string fails to decode).
    .bind(id)
    .bind(scope)
    .bind(jsonpath)
    .bind(applies)
    .execute(pool)
    .await
    .expect("insert redaction policy");
    id
}

#[tokio::test]
async fn load_all_empty() {
    let (pool, _guard) = test_setup().await;
    let repo = PgHotlRedactionRepo::new(pool.clone());

    let rows = repo.load_all().await.expect("load empty");
    assert!(
        rows.is_empty(),
        "expected empty Vec when no policies exist, got {} rows",
        rows.len()
    );
}

#[tokio::test]
async fn load_all_returns_inserted_rows() {
    let (pool, _guard) = test_setup().await;
    let repo = PgHotlRedactionRepo::new(pool.clone());

    insert_policy(&pool, "tool_call.execute_python", "$.password", &["sse"]).await;
    insert_policy(
        &pool,
        "tool_call.http_get",
        "$.headers.authorization",
        &["sse", "audit"],
    )
    .await;

    let rows = repo.load_all().await.expect("load");
    assert_eq!(rows.len(), 2, "expected 2 policies, got {}", rows.len());
}

#[tokio::test]
async fn load_all_sorts_exact_scope_before_wildcard() {
    let (pool, _guard) = test_setup().await;
    let repo = PgHotlRedactionRepo::new(pool.clone());

    // Insert wildcard first to verify the ORDER BY isn't an insertion artefact.
    insert_policy(&pool, "*", "$.catch_all", &["sse"]).await;
    insert_policy(&pool, "tool_call.execute_python", "$.password", &["sse"]).await;

    let rows = repo.load_all().await.expect("load");
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].scope, "tool_call.execute_python",
        "exact scope must precede '*' so S13-4 picks the most specific match first"
    );
    assert_eq!(rows[1].scope, "*", "catch-all '*' must come last");
}
