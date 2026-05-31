//! Sprint-12 S12-7 — PG round-trip coverage for `PgHotlDecisionStore` and
//! `PgHotlAuditSink`.
//!
//! Three cases:
//!
//!   1. `pg_create_then_find_round_trip` — record a decision, then snapshot
//!      via a sibling `SELECT` and assert the row shape matches what the
//!      store returned.
//!   2. `pg_create_duplicate_request_id_returns_conflict` — second insert
//!      with the same `request_id` must surface `HotlDecisionStoreError::Duplicate`
//!      (the UNIQUE constraint on the column).
//!   3. `pg_audit_sink_writes_then_visible_to_downstream_reader` — `PgHotlAuditSink::append`
//!      delegates to `xiaoguai_audit::PgAuditSink::append`, so the row must
//!      be visible via the same sink's `list` reader (which is what the
//!      `/v1/admin/audit` route uses in production).
//!
//! These tests are `#[ignore]` and require a live PG via `DATABASE_URL`,
//! matching the pattern in `outcomes_bridge.rs` / `hotl_bridge.rs::tests`.
//! CI runs them via the existing PG fixture.

#![cfg(test)]

use sqlx::PgPool;
use uuid::Uuid;
use xiaoguai_api::hotl::audit::HotlAuditSink;
use xiaoguai_api::hotl::decision::{
    HotlDecisionStore, HotlDecisionStoreError, HotlDecisionVerdict,
};
use xiaoguai_audit::AuditEntry;
use xiaoguai_core::hotl_bridge::{PgHotlAuditSink, PgHotlDecisionStore};

async fn pg_pool() -> PgPool {
    let url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for PG integration tests");
    PgPool::connect(&url).await.expect("pg connect")
}

#[tokio::test]
#[ignore = "requires live PG; run with DATABASE_URL set"]
async fn pg_create_then_find_round_trip() {
    let pool = pg_pool().await;
    let store = PgHotlDecisionStore::new(pool.clone());

    let request_id = Uuid::new_v4();
    let tenant_id = Uuid::new_v4();
    let policy_id = Uuid::new_v4();

    let recorded = store
        .record(
            request_id,
            tenant_id,
            HotlDecisionVerdict::Allow,
            "alice@example.com".into(),
            Some(policy_id),
        )
        .await
        .expect("record should succeed");

    assert_eq!(recorded.request_id, request_id);
    assert_eq!(recorded.tenant_id, tenant_id);
    assert_eq!(recorded.verdict, HotlDecisionVerdict::Allow);
    assert_eq!(recorded.decided_by, "alice@example.com");
    assert_eq!(recorded.raised_policy_id, Some(policy_id));

    // Cross-check by selecting the row directly.
    let row: (Uuid, Uuid, String, String, Option<Uuid>) = sqlx::query_as(
        "SELECT id, tenant_id, verdict, decided_by, raised_policy_id \
         FROM hotl_decisions WHERE request_id = $1",
    )
    .bind(request_id)
    .fetch_one(&pool)
    .await
    .expect("row must exist");

    assert_eq!(row.0, recorded.id);
    assert_eq!(row.1, tenant_id);
    assert_eq!(row.2, "allow");
    assert_eq!(row.3, "alice@example.com");
    assert_eq!(row.4, Some(policy_id));

    // Cleanup.
    sqlx::query("DELETE FROM hotl_decisions WHERE request_id = $1")
        .bind(request_id)
        .execute(&pool)
        .await
        .ok();
}

#[tokio::test]
#[ignore = "requires live PG; run with DATABASE_URL set"]
async fn pg_create_duplicate_request_id_returns_conflict() {
    let pool = pg_pool().await;
    let store = PgHotlDecisionStore::new(pool.clone());

    let request_id = Uuid::new_v4();
    let tenant_id = Uuid::new_v4();

    store
        .record(
            request_id,
            tenant_id,
            HotlDecisionVerdict::Allow,
            "alice".into(),
            None,
        )
        .await
        .expect("first record");

    let err = store
        .record(
            request_id,
            tenant_id,
            HotlDecisionVerdict::Deny,
            "bob".into(),
            None,
        )
        .await
        .expect_err("duplicate must error");

    assert!(
        matches!(err, HotlDecisionStoreError::Duplicate(id) if id == request_id),
        "expected Duplicate({request_id}), got {err:?}"
    );

    sqlx::query("DELETE FROM hotl_decisions WHERE request_id = $1")
        .bind(request_id)
        .execute(&pool)
        .await
        .ok();
}

#[tokio::test]
#[ignore = "requires live PG; run with DATABASE_URL set"]
async fn pg_audit_sink_writes_then_visible_to_downstream_reader() {
    let pool = pg_pool().await;
    let signing_key = b"sprint12-s12-7-integration-test-key".to_vec();
    let pg_sink = std::sync::Arc::new(xiaoguai_audit::chain::sink::PgAuditSink::new(
        pool.clone(),
        signing_key,
    ));
    let hotl_sink = PgHotlAuditSink::new(pg_sink.clone());

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

    let rows = pg_sink
        .list(&tenant_id, None, None, 10)
        .await
        .expect("read back");

    assert_eq!(rows.len(), 1, "expected exactly one row for fresh tenant");
    let stored = &rows[0];
    assert_eq!(stored.entry.tenant_id, tenant_id);
    assert_eq!(stored.entry.action, "hotl.decision");
    assert_eq!(stored.entry.actor, "alice@example.com");
    assert_eq!(
        stored.entry.resource.as_deref(),
        Some(format!("escalation:{request_id}").as_str())
    );

    // Cleanup audit_log rows for this tenant (each test gets a unique id so
    // we never collide, but be polite).
    sqlx::query("DELETE FROM audit_log WHERE tenant_id = $1")
        .bind(&tenant_id)
        .execute(&pool)
        .await
        .ok();
}
