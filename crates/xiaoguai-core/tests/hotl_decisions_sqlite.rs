//! Sprint-12 S12-7 — `SQLite` round-trip coverage for `SqliteHotlDecisionStore`
//! and `SqliteHotlAuditSink`.
//!
//! `DEC-033` (single-user `SQLite` pivot): these were `#[ignore]`'d Postgres
//! tests requiring `DATABASE_URL`. They now run against a temp `SQLite` DB on
//! every `cargo test`. The `hotl_decisions` table lost its `tenant_id` column
//! (the idempotency key is `request_id`); the synthesized `tenant_id` on the
//! returned record is `Uuid::nil()`. The audit sink ignores `tenant_id` on
//! both write and read and synthesizes `OWNER_TENANT_ID` on read.
//!
//! Three cases:
//!
//!   1. `create_then_find_round_trip` — record a decision, then snapshot via
//!      a sibling `SELECT` and assert the row shape matches.
//!   2. `create_duplicate_request_id_returns_conflict` — second insert with
//!      the same `request_id` must surface `HotlDecisionStoreError::Duplicate`.
//!   3. `audit_sink_writes_then_visible_to_downstream_reader` —
//!      `SqliteHotlAuditSink::append` delegates to `SqliteAuditSink::append`, so the
//!      row must be visible via the same sink's `list` reader.

#![cfg(test)]

use sqlx::SqlitePool;
use uuid::Uuid;
use xiaoguai_api::hotl::audit::HotlAuditSink;
use xiaoguai_api::hotl::decision::{
    HotlDecisionStore, HotlDecisionStoreError, HotlDecisionVerdict,
};
use xiaoguai_audit::AuditEntry;
use xiaoguai_core::hotl_bridge::{SqliteHotlAuditSink, SqliteHotlDecisionStore};

async fn sqlite_pool() -> (tempfile::TempDir, SqlitePool) {
    let dir = tempfile::tempdir().unwrap();
    let pool = xiaoguai_storage::db::connect(dir.path().join("t.db").to_str().unwrap(), 5)
        .await
        .unwrap();
    xiaoguai_storage::db::migrate(&pool).await.unwrap();
    (dir, pool)
}

#[tokio::test]
async fn create_then_find_round_trip() {
    let (_dir, pool) = sqlite_pool().await;
    let store = SqliteHotlDecisionStore::new(pool.clone());

    let request_id = Uuid::new_v4();
    let policy_id = Uuid::new_v4();

    let recorded = store
        .record(
            request_id,
            HotlDecisionVerdict::Allow,
            "alice@example.com".into(),
            Some(policy_id),
        )
        .await
        .expect("record should succeed");

    assert_eq!(recorded.request_id, request_id);
    assert_eq!(recorded.verdict, HotlDecisionVerdict::Allow);
    assert_eq!(recorded.decided_by, "alice@example.com");
    assert_eq!(recorded.raised_policy_id, Some(policy_id));

    // Cross-check by selecting the row directly. No tenant_id column under
    // DEC-033; bind via `?` placeholders.
    let row: (Uuid, String, String, Option<Uuid>) = sqlx::query_as(
        "SELECT id, verdict, decided_by, raised_policy_id \
         FROM hotl_decisions WHERE request_id = ?",
    )
    .bind(request_id)
    .fetch_one(&pool)
    .await
    .expect("row must exist");

    assert_eq!(row.0, recorded.id);
    assert_eq!(row.1, "allow");
    assert_eq!(row.2, "alice@example.com");
    assert_eq!(row.3, Some(policy_id));
}

#[tokio::test]
async fn create_duplicate_request_id_returns_conflict() {
    let (_dir, pool) = sqlite_pool().await;
    let store = SqliteHotlDecisionStore::new(pool.clone());

    let request_id = Uuid::new_v4();

    store
        .record(request_id, HotlDecisionVerdict::Allow, "alice".into(), None)
        .await
        .expect("first record");

    let err = store
        .record(request_id, HotlDecisionVerdict::Deny, "bob".into(), None)
        .await
        .expect_err("duplicate must error");

    assert!(
        matches!(err, HotlDecisionStoreError::Duplicate(id) if id == request_id),
        "expected Duplicate({request_id}), got {err:?}"
    );
}

#[tokio::test]
async fn audit_sink_writes_then_visible_to_downstream_reader() {
    let (_dir, pool) = sqlite_pool().await;
    let signing_key = b"sprint12-s12-7-integration-test-key".to_vec();
    let pg_sink = std::sync::Arc::new(xiaoguai_audit::chain::sink::SqliteAuditSink::new(
        pool.clone(),
        signing_key,
    ));
    let hotl_sink = SqliteHotlAuditSink::new(pg_sink.clone());

    // tenant_id is vestigial under DEC-033 (ignored on write/read).
    let tenant_id = format!("ten_{}", Uuid::new_v4().simple());
    let request_id = Uuid::new_v4();
    let entry = AuditEntry {
        ts: chrono::Utc::now(),
        tenant_id: tenant_id.clone(),
        actor: "alice@example.com".into(),
        action: "hotl.decision".into(),
        resource: Some(format!("escalation:{request_id}")),
        details: serde_json::json!({
            "verdict": "allow",
            "request_id": request_id,
        }),
    };

    hotl_sink
        .append(entry.clone())
        .await
        .expect("audit append must succeed");

    // The temp DB starts empty, so the single append is the only row.
    let rows = pg_sink
        .list(&tenant_id, None, None, 10)
        .await
        .expect("read back");

    assert_eq!(rows.len(), 1, "expected exactly one row in the fresh DB");
    let stored = &rows[0];
    // DEC-033: tenant_id column dropped; reader synthesizes the owner id.
    assert_eq!(stored.entry.tenant_id, xiaoguai_audit::OWNER_TENANT_ID);
    assert_eq!(stored.entry.action, "hotl.decision");
    assert_eq!(stored.entry.actor, "alice@example.com");
    assert_eq!(
        stored.entry.resource.as_deref(),
        Some(format!("escalation:{request_id}").as_str())
    );
}
