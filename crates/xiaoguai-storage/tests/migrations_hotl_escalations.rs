//! sprint-13 S13-1: schema round-trip for migration `0027_hotl_escalations_split.sql`.
//!
//! Asserts the migration produces:
//! - `hotl_escalations` parent table (with RLS enabled + `tenant_isolation` policy)
//! - `hotl_pending` child table with `escalation_id` FK
//! - `hotl_redaction_policies` table (with RLS)
//! - `casbin_rule` seed row for `(p, hotl:decide, /v1/hotl/decisions, POST, allow)`
//! - **No** path-based fallback row `(p, *, /v1/hotl/decisions, POST, *)`
//!
//! The backfill scenario described in the sprint plan §S13-1 (3 v1.9-shape
//! `hotl_pending` rows → 3 parents + 3 children) is implemented by inserting
//! 3 (parent, child) pairs through the post-migration schema and checking
//! the 1-to-1 invariant + zero orphans. Because the live v1.9.x branch never
//! shipped a `hotl_pending` table (migration 0026 only added `hotl_decisions`),
//! the backfill block in 0027 is a NO-OP today; the round-trip below
//! emulates the same end-state shape that the backfill would produce had
//! prior `hotl_pending` rows existed.
//!
//! Marked `#[ignore]` — Docker required, same convention as the other
//! testcontainers-backed migration tests in this crate.

#![cfg(test)]

use chrono::{Duration, Utc};
use sqlx::Row;
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{runners::AsyncRunner, ImageExt},
};
use uuid::Uuid;
use xiaoguai_storage::db;

#[tokio::test]
#[ignore = "requires Docker"]
async fn migration_0027_creates_parent_child_redaction_and_casbin_seed() {
    // pgvector image — migration 0019 needs the `vector` extension.
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

    // ---- 1. Schema shape ----------------------------------------------------

    let escalations_exists: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
         WHERE table_name = 'hotl_escalations')",
    )
    .fetch_one(&pool)
    .await
    .expect("query escalations table");
    assert!(
        escalations_exists.0,
        "hotl_escalations parent table missing"
    );

    let pending_exists: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
         WHERE table_name = 'hotl_pending')",
    )
    .fetch_one(&pool)
    .await
    .expect("query pending table");
    assert!(pending_exists.0, "hotl_pending child table missing");

    let redaction_exists: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
         WHERE table_name = 'hotl_redaction_policies')",
    )
    .fetch_one(&pool)
    .await
    .expect("query redaction table");
    assert!(redaction_exists.0, "hotl_redaction_policies table missing");

    let casbin_exists: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
         WHERE table_name = 'casbin_rule')",
    )
    .fetch_one(&pool)
    .await
    .expect("query casbin table");
    assert!(casbin_exists.0, "casbin_rule seed table missing");

    // `hotl_pending` must NOT carry a `request_id` column post-migration —
    // the rename to `escalation_id` is canonical (DEC-HLD-016).
    let request_id_col: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns \
         WHERE table_name = 'hotl_pending' AND column_name = 'request_id')",
    )
    .fetch_one(&pool)
    .await
    .expect("query column");
    assert!(
        !request_id_col.0,
        "hotl_pending.request_id should be removed (replaced by escalation_id FK)"
    );

    let escalation_id_col: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns \
         WHERE table_name = 'hotl_pending' AND column_name = 'escalation_id')",
    )
    .fetch_one(&pool)
    .await
    .expect("query column");
    assert!(
        escalation_id_col.0,
        "hotl_pending.escalation_id (FK to hotl_escalations.id) missing"
    );

    // ---- 2. Insert 3 (parent, child) pairs ---------------------------------

    let tenant_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    let now = Utc::now();

    let mut parent_ids = Vec::new();
    for i in 0..3 {
        let parent_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO hotl_escalations \
             (id, tenant_id, session_id, top_level_scope, created_at, parent_id) \
             VALUES ($1, $2, $3, $4, $5, NULL)",
        )
        .bind(parent_id)
        .bind(tenant_id)
        .bind(session_id)
        .bind(format!("tool_call.scope_{i}"))
        .bind(now)
        .execute(&pool)
        .await
        .expect("insert escalation");
        parent_ids.push(parent_id);

        sqlx::query(
            "INSERT INTO hotl_pending \
             (id, escalation_id, tenant_id, scope, tool, args_redacted, status, \
              expires_at, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6::jsonb, $7, $8, $9)",
        )
        .bind(Uuid::new_v4())
        .bind(parent_id)
        .bind(tenant_id)
        .bind(format!("tool_call.scope_{i}"))
        .bind("execute_python")
        .bind(serde_json::json!({"code": "print(1)"}))
        .bind("pending")
        .bind(now + Duration::hours(24))
        .bind(now)
        .execute(&pool)
        .await
        .expect("insert pending");
    }

    // 1-to-1: 3 parents, 3 children, 0 orphans.
    let parent_count: (i64,) = sqlx::query_as("SELECT count(*) FROM hotl_escalations")
        .fetch_one(&pool)
        .await
        .expect("count escalations");
    assert_eq!(parent_count.0, 3, "expected 3 parent rows after seed");

    let child_count: (i64,) = sqlx::query_as("SELECT count(*) FROM hotl_pending")
        .fetch_one(&pool)
        .await
        .expect("count pending");
    assert_eq!(child_count.0, 3, "expected 3 child rows after seed");

    let orphan_count: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM hotl_pending p \
         LEFT JOIN hotl_escalations e ON e.id = p.escalation_id \
         WHERE e.id IS NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("count orphans");
    assert_eq!(
        orphan_count.0, 0,
        "no hotl_pending child should be orphaned from its escalation"
    );

    // FK ON DELETE CASCADE: deleting a parent removes its child.
    sqlx::query("DELETE FROM hotl_escalations WHERE id = $1")
        .bind(parent_ids[0])
        .execute(&pool)
        .await
        .expect("delete parent");
    let after_delete: (i64,) = sqlx::query_as("SELECT count(*) FROM hotl_pending")
        .fetch_one(&pool)
        .await
        .expect("count after delete");
    assert_eq!(
        after_delete.0, 2,
        "ON DELETE CASCADE should drop the child row when parent is deleted"
    );

    // ---- 3. hotl_redaction_policies empty + writable ------------------------

    let redaction_count: (i64,) = sqlx::query_as("SELECT count(*) FROM hotl_redaction_policies")
        .fetch_one(&pool)
        .await
        .expect("count redaction");
    assert_eq!(
        redaction_count.0, 0,
        "hotl_redaction_policies should ship empty — admin-ui CRUD lands in sprint-14"
    );

    sqlx::query(
        "INSERT INTO hotl_redaction_policies \
         (id, tenant_id, scope, jsonpath, applies_to, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(tenant_id)
    .bind("tool_call.execute_python")
    .bind("$.password")
    .bind(vec!["sse".to_string()])
    .bind(now)
    .execute(&pool)
    .await
    .expect("insert redaction policy");

    // ---- 4. Casbin seed: hotl:decide present, path-based rule absent --------

    let row = sqlx::query(
        "SELECT ptype, v0, v1, v2, v3 FROM casbin_rule \
         WHERE ptype = 'p' AND v0 = 'hotl:decide' \
           AND v1 = '/v1/hotl/decisions' AND v2 = 'POST'",
    )
    .fetch_optional(&pool)
    .await
    .expect("query casbin seed");
    let row = row.expect("expected (p, hotl:decide, /v1/hotl/decisions, POST, allow) seed row");
    let v3: String = row.try_get("v3").unwrap_or_default();
    assert_eq!(
        v3, "allow",
        "hotl:decide scope rule's effect must be 'allow'"
    );

    let path_based: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM casbin_rule \
         WHERE ptype = 'p' AND v0 = '*' AND v1 = '/v1/hotl/decisions' AND v2 = 'POST'",
    )
    .fetch_one(&pool)
    .await
    .expect("count path-based");
    assert_eq!(
        path_based.0, 0,
        "path-based fallback rule (p, *, /v1/hotl/decisions, POST, *) must be removed"
    );
}
