//! sprint-13 S13-1: schema round-trip for migration `0027_hotl_escalations_split.sql`.
//!
//! Embedded `SQLite` (DEC-033). No Docker — `common::test_setup` applies every
//! migration to a temp database. Asserts the migration produces:
//! - `hotl_escalations` parent table + `hotl_pending` child with `escalation_id` FK
//! - `hotl_redaction_policies` table (ships empty, writable)
//! - `casbin_rule` seed row for `(p, hotl:decide, /v1/hotl/decisions, POST, allow)`
//! - **No** path-based fallback row `(p, *, /v1/hotl/decisions, POST, *)`
//!
//! Under the single-user pivot the `tenant_id` columns + RLS are dropped, UUIDs
//! are TEXT, and JSON payloads (`args_redacted`, `applies_to`) are TEXT.

mod common;

use chrono::{Duration, Utc};
use common::test_setup;
use sqlx::Row;
use uuid::Uuid;

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

/// Whether a column exists on a table (via `PRAGMA table_info`).
async fn column_exists(pool: &sqlx::SqlitePool, table: &str, column: &str) -> bool {
    let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(pool)
        .await
        .expect("pragma table_info");
    rows.iter().any(|r| {
        let name: String = r.try_get("name").unwrap_or_default();
        name == column
    })
}

#[tokio::test]
async fn migration_0027_creates_parent_child_redaction_and_casbin_seed() {
    let (pool, _guard) = test_setup().await;

    // ---- 1. Schema shape ----------------------------------------------------

    assert!(
        table_exists(&pool, "hotl_escalations").await,
        "hotl_escalations parent table missing"
    );
    assert!(
        table_exists(&pool, "hotl_pending").await,
        "hotl_pending child table missing"
    );
    assert!(
        table_exists(&pool, "hotl_redaction_policies").await,
        "hotl_redaction_policies table missing"
    );
    assert!(
        table_exists(&pool, "casbin_rule").await,
        "casbin_rule seed table missing"
    );

    // `hotl_pending` must NOT carry a `request_id` column post-migration —
    // the rename to `escalation_id` is canonical (DEC-HLD-016).
    assert!(
        !column_exists(&pool, "hotl_pending", "request_id").await,
        "hotl_pending.request_id should be removed (replaced by escalation_id FK)"
    );
    assert!(
        column_exists(&pool, "hotl_pending", "escalation_id").await,
        "hotl_pending.escalation_id (FK to hotl_escalations.id) missing"
    );
    // tenant_id columns are dropped under the single-user pivot.
    assert!(
        !column_exists(&pool, "hotl_pending", "tenant_id").await,
        "hotl_pending.tenant_id should be dropped under the pivot"
    );
    assert!(
        !column_exists(&pool, "hotl_escalations", "tenant_id").await,
        "hotl_escalations.tenant_id should be dropped under the pivot"
    );

    // ---- 2. Insert 3 (parent, child) pairs ---------------------------------

    let now = Utc::now();
    let mut parent_ids = Vec::new();
    for i in 0..3 {
        let parent_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO hotl_escalations \
             (id, session_id, top_level_scope, created_at, parent_id) \
             VALUES (?, ?, ?, ?, NULL)",
        )
        .bind(parent_id.to_string())
        .bind(Uuid::new_v4().to_string())
        .bind(format!("tool_call.scope_{i}"))
        .bind(now)
        .execute(&pool)
        .await
        .expect("insert escalation");
        parent_ids.push(parent_id);

        sqlx::query(
            "INSERT INTO hotl_pending \
             (id, escalation_id, scope, tool, args_redacted, status, \
              expires_at, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(parent_id.to_string())
        .bind(format!("tool_call.scope_{i}"))
        .bind("execute_python")
        .bind(serde_json::json!({"code": "print(1)"}).to_string())
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
    sqlx::query("DELETE FROM hotl_escalations WHERE id = ?")
        .bind(parent_ids[0].to_string())
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
         (id, scope, jsonpath, applies_to, created_at) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind("tool_call.execute_python")
    .bind("$.password")
    .bind(serde_json::json!(["sse"]).to_string())
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
    assert_eq!(v3, "allow", "hotl:decide scope rule's effect must be 'allow'");

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
