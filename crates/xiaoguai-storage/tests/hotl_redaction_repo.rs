//! sprint-13 S13-3: read-only `HotlRedactionRepo::load_for_tenant` round-trip.
//!
//! Asserts the repo:
//! - returns an empty Vec when no policies exist for the tenant,
//! - returns all rows for the tenant (and only those rows — RLS isolation),
//! - sorts results so exact-scope rules come before the `*` catch-all
//!   (caller in S13-4 picks the most specific match first).
//!
//! Uses the same `testcontainers` + `pgvector/pgvector:pg16` pattern as
//! `migrations_hotl_escalations.rs` (PR #138). Marked `#[ignore]` — Docker
//! required, opt-in via `cargo test -- --ignored`.

#![cfg(test)]

use chrono::Utc;
use sqlx::Executor;
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{runners::AsyncRunner, ImageExt},
};
use uuid::Uuid;
use xiaoguai_storage::{
    db,
    repositories::hotl_redaction::{HotlRedactionRepo, PgHotlRedactionRepo},
};

/// Insert a redaction policy via raw SQL (the repo is read-only).
async fn insert_policy(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    scope: &str,
    jsonpath: &str,
    applies_to: &[&str],
) -> Uuid {
    let id = Uuid::new_v4();
    let applies: Vec<String> = applies_to.iter().map(|s| (*s).to_string()).collect();
    sqlx::query(
        "INSERT INTO hotl_redaction_policies \
         (id, tenant_id, scope, jsonpath, applies_to, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(tenant_id)
    .bind(scope)
    .bind(jsonpath)
    .bind(applies)
    .bind(Utc::now())
    .execute(pool)
    .await
    .expect("insert redaction policy");
    id
}

async fn start_pg() -> sqlx::PgPool {
    let pg = Postgres::default()
        .with_name("pgvector/pgvector")
        .with_tag("pg16")
        .start()
        .await
        .expect("start pg");
    let port = pg.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = db::connect(&url, 5).await.expect("connect");
    db::migrate(&pool).await.expect("migrate");

    // Leak the container so it stays up for the duration of the test —
    // dropping it would tear down the database mid-query.
    std::mem::forget(pg);
    pool
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn load_for_tenant_empty() {
    let pool = start_pg().await;
    let repo = PgHotlRedactionRepo::new(pool.clone());

    let tenant_id = Uuid::new_v4();
    let rows = repo
        .load_for_tenant(tenant_id)
        .await
        .expect("load empty tenant");
    assert!(
        rows.is_empty(),
        "expected empty Vec for tenant with no policies, got {} rows",
        rows.len()
    );
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn load_for_tenant_returns_inserted_rows() {
    let pool = start_pg().await;
    let repo = PgHotlRedactionRepo::new(pool.clone());

    let tenant_a = Uuid::new_v4();
    let tenant_b = Uuid::new_v4();

    insert_policy(
        &pool,
        tenant_a,
        "tool_call.execute_python",
        "$.password",
        &["sse"],
    )
    .await;
    insert_policy(
        &pool,
        tenant_a,
        "tool_call.http_get",
        "$.headers.authorization",
        &["sse", "audit"],
    )
    .await;
    insert_policy(&pool, tenant_b, "*", "$.token", &["sse"]).await;

    let rows = repo.load_for_tenant(tenant_a).await.expect("load tenant_a");
    assert_eq!(
        rows.len(),
        2,
        "expected 2 policies for tenant_a, got {}",
        rows.len()
    );
    for row in &rows {
        assert_eq!(row.tenant_id, tenant_a, "row tenant_id mismatch");
    }
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn load_for_tenant_sorts_exact_scope_before_wildcard() {
    let pool = start_pg().await;
    let repo = PgHotlRedactionRepo::new(pool.clone());

    let tenant_id = Uuid::new_v4();

    // Insert wildcard first to verify the ORDER BY isn't an insertion artefact.
    insert_policy(&pool, tenant_id, "*", "$.catch_all", &["sse"]).await;
    insert_policy(
        &pool,
        tenant_id,
        "tool_call.execute_python",
        "$.password",
        &["sse"],
    )
    .await;

    let rows = repo.load_for_tenant(tenant_id).await.expect("load tenant");
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].scope, "tool_call.execute_python",
        "exact scope must precede '*' so S13-4 picks the most specific match first"
    );
    assert_eq!(rows[1].scope, "*", "catch-all '*' must come last");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn load_for_tenant_respects_rls() {
    // Verifies that even if rows for two tenants coexist, the repo only
    // returns rows for the tenant it was asked about. RLS is set inside the
    // repo via `app.current_tenant_id`; this test confirms the GUC is wired.
    let pool = start_pg().await;

    // Force RLS for the test role. The `postgres` superuser bypasses
    // non-FORCE policies, so we toggle FORCE to prove RLS is effective.
    pool.execute("ALTER TABLE hotl_redaction_policies FORCE ROW LEVEL SECURITY")
        .await
        .expect("force RLS");

    let repo = PgHotlRedactionRepo::new(pool.clone());

    let tenant_a = Uuid::new_v4();
    let tenant_b = Uuid::new_v4();

    insert_policy(&pool, tenant_a, "tool_call.a", "$.a", &["sse"]).await;
    insert_policy(&pool, tenant_a, "tool_call.b", "$.b", &["sse"]).await;
    insert_policy(&pool, tenant_b, "tool_call.x", "$.x", &["sse"]).await;

    let a_rows = repo.load_for_tenant(tenant_a).await.expect("load tenant_a");
    assert_eq!(a_rows.len(), 2, "tenant_a should see exactly its 2 rows");
    for row in &a_rows {
        assert_eq!(row.tenant_id, tenant_a);
    }

    let b_rows = repo.load_for_tenant(tenant_b).await.expect("load tenant_b");
    assert_eq!(b_rows.len(), 1, "tenant_b should see exactly its 1 row");
    assert_eq!(b_rows[0].tenant_id, tenant_b);
}
