//! sprint-13 S13-2: integration tests for `HotlEscalationRepo` / `HotlEscalationStore`.
//!
//! Backs the boot-replay path used by `xiaoguai-core::run_serve` (S13-5): a
//! restart of `xiaoguai-api` must reattach every pending `HotL` waiter that is
//! still within its `expires_at` window, and any UPDATE-on-decision must
//! return whether a pending row actually matched (so the registry can fall
//! back to `verdict=timeout` for stale ids).
//!
//! Embedded `SQLite` (DEC-033). No Docker — each test opens a temp database via
//! `common::test_setup`. Single-owner deployment: no tenant scoping.

mod common;

use chrono::{Duration, Utc};
use common::test_setup;
use uuid::Uuid;
use xiaoguai_storage::repositories::hotl_escalations::{
    HotlEscalationRow, HotlEscalationStore, HotlPendingRow, SqliteHotlEscalationRepository,
};
use xiaoguai_storage::repositories::HotlDecisionVerdict;

fn make_parent(scope: &str) -> HotlEscalationRow {
    HotlEscalationRow {
        id: Uuid::new_v4(),
        session_id: Uuid::new_v4(),
        top_level_scope: scope.to_string(),
        status: "pending".to_string(),
        created_at: Utc::now(),
        parent_id: None,
    }
}

fn make_child(scope: &str, expires_in: Duration) -> HotlPendingRow {
    let now = Utc::now();
    HotlPendingRow {
        id: Uuid::new_v4(),
        // `escalation_id` is overwritten by `insert_pending` with the parent id
        // it actually persisted; the value provided here is irrelevant.
        escalation_id: Uuid::nil(),
        scope: scope.to_string(),
        tool: "execute_python".to_string(),
        args_redacted: serde_json::json!({"code": "print(1)"}),
        status: "pending".to_string(),
        expires_at: now + expires_in,
        created_at: now,
        decided_at: None,
        decided_by: None,
    }
}

#[tokio::test]
async fn insert_pending_round_trip() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteHotlEscalationRepository::new(pool.clone());
    let parent = make_parent("tool_call.execute_python");
    let child = make_child("tool_call.execute_python", Duration::hours(24));

    let escalation_id = repo
        .insert_pending(parent.clone(), child.clone())
        .await
        .expect("insert_pending should succeed");

    // The returned id is the parent id that the child row now FK-references.
    assert_eq!(
        escalation_id, parent.id,
        "insert_pending should return the parent id used as escalation_id"
    );

    let rows = repo
        .list_pending_unexpired(Utc::now())
        .await
        .expect("list_pending_unexpired should succeed");

    assert_eq!(rows.len(), 1, "should see the one row we inserted");
    assert_eq!(rows[0].escalation_id, escalation_id);
    assert_eq!(rows[0].scope, "tool_call.execute_python");
    assert_eq!(rows[0].tool, "execute_python");
    assert_eq!(rows[0].status, "pending");
}

#[tokio::test]
async fn list_pending_unexpired_excludes_expired() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteHotlEscalationRepository::new(pool.clone());
    let parent = make_parent("tool_call.execute_python");
    // expires_at = now - 1m → already expired by the time list runs.
    let child = make_child("tool_call.execute_python", Duration::minutes(-1));

    repo.insert_pending(parent, child)
        .await
        .expect("insert_pending should succeed");

    let rows = repo
        .list_pending_unexpired(Utc::now())
        .await
        .expect("list_pending_unexpired should succeed");

    assert!(
        rows.is_empty(),
        "expired rows must not appear in boot replay (got {} rows)",
        rows.len()
    );
}

#[tokio::test]
async fn list_pending_unexpired_excludes_decided() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteHotlEscalationRepository::new(pool.clone());
    let parent = make_parent("tool_call.execute_python");
    let mut child = make_child("tool_call.execute_python", Duration::hours(24));
    // Pre-mark child as already-resolved; list must skip it even though
    // expires_at is in the future.
    child.status = "resolved".to_string();

    repo.insert_pending(parent, child)
        .await
        .expect("insert_pending should succeed");

    let rows = repo
        .list_pending_unexpired(Utc::now())
        .await
        .expect("list_pending_unexpired should succeed");

    assert!(
        rows.is_empty(),
        "decided rows must not appear in boot replay (got {} rows)",
        rows.len()
    );
}

#[tokio::test]
async fn record_decision_resolves_pending_row() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteHotlEscalationRepository::new(pool.clone());
    let parent = make_parent("tool_call.execute_python");
    let child = make_child("tool_call.execute_python", Duration::hours(24));

    let escalation_id = repo
        .insert_pending(parent, child)
        .await
        .expect("insert_pending should succeed");

    let matched = repo
        .record_decision(
            escalation_id,
            HotlDecisionVerdict::Allowed,
            Some("operator@example.com".to_string()),
        )
        .await
        .expect("record_decision should succeed");

    assert!(matched, "record_decision must report a matched row");

    let rows = repo
        .list_pending_unexpired(Utc::now())
        .await
        .expect("list_pending_unexpired should succeed");
    assert!(
        rows.is_empty(),
        "resolved row must not appear in boot replay after record_decision"
    );

    // A second record_decision on the same id is a no-op (row is no longer pending).
    let matched_again = repo
        .record_decision(escalation_id, HotlDecisionVerdict::Allowed, None)
        .await
        .expect("record_decision (idempotent) should succeed");
    assert!(
        !matched_again,
        "record_decision on already-resolved row must return false"
    );
}

#[tokio::test]
async fn record_decision_rejects_expired_row() {
    // Security regression: a timed-out escalation stays `status='pending'` (the
    // in-memory timeout path doesn't terminalise the DB row), so without the
    // `expires_at > now` guard a late operator decision would falsely flip an
    // already-abandoned escalation to 'resolved'. The guard must reject it.
    let (pool, _guard) = test_setup().await;
    let repo = SqliteHotlEscalationRepository::new(pool.clone());
    let parent = make_parent("tool_call.execute_python");
    let child = make_child("tool_call.execute_python", Duration::minutes(-1)); // already expired

    let escalation_id = repo
        .insert_pending(parent, child)
        .await
        .expect("insert_pending should succeed");

    let matched = repo
        .record_decision(escalation_id, HotlDecisionVerdict::Allowed, None)
        .await
        .expect("record_decision should succeed");
    assert!(
        !matched,
        "record_decision on an expired (timed-out) row must return false, not stamp it resolved"
    );
}

#[tokio::test]
async fn record_decision_unknown_id_returns_false() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteHotlEscalationRepository::new(pool.clone());

    let unknown = Uuid::new_v4();
    let matched = repo
        .record_decision(unknown, HotlDecisionVerdict::Denied, None)
        .await
        .expect("record_decision should succeed even on unknown id");

    assert!(
        !matched,
        "record_decision on unknown escalation_id must return false"
    );
}
