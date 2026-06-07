//! Sprint-13 S13-5 — `DecisionRegistry` persistence + boot-time replay.
//!
//! These tests pin the new contract introduced by S13-5:
//!
//! 1. `register` persists the (parent, child) pair via
//!    `HotlEscalationStore::insert_pending` BEFORE inserting the in-memory
//!    oneshot sender. A persist failure leaves zero in-memory state.
//! 2. `resolve` persists the verdict via
//!    `HotlEscalationStore::record_decision` BEFORE firing the oneshot.
//!    A store miss (`Ok(false)`) maps to `Err(UnknownEscalation)` so the
//!    route handler can render 404.
//! 3. `replay_from_storage` rebuilds the waiter map from
//!    `list_pending_unexpired`, minting a fresh oneshot per row and
//!    spawning a `sleep_until(expires_at)` companion that emits
//!    `verdict=timeout` on fire. The replay is observable via a new
//!    `xiaoguai_hotl_registry_replayed_total{outcome}` counter.
//!
//! No PG in this test: a `MockHotlEscalationStore` (DashMap-backed)
//! stands in for `SqliteHotlEscalationRepository`. Integration coverage
//! against the live PG schema lives in the testcontainers suite under
//! `xiaoguai-storage/tests/`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde_json::json;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use xiaoguai_api::hotl::decision_registry::{DecisionRegistry, HotlResolution, RegistryError};
use xiaoguai_storage::repositories::error::{RepoError, RepoResult};
use xiaoguai_storage::repositories::hotl_escalations::{
    HotlDecisionVerdict as StoreVerdict, HotlEscalationRow, HotlEscalationStore, HotlPendingRow,
};

// ── mock store ────────────────────────────────────────────────────────────────

/// In-memory `HotlEscalationStore` for unit-level tests of S13-5.
///
/// Stores child rows keyed by `escalation_id` so `list_pending_unexpired`
/// and `record_decision` can roundtrip without touching Postgres. Failure
/// injection is via the `fail_insert` flag — when set, `insert_pending`
/// returns `RepoError::Database` to drive the "persist failure leaves no
/// in-memory waiter" assertion.
#[derive(Debug, Default)]
struct MockHotlEscalationStore {
    pending: DashMap<Uuid, HotlPendingRow>,
    fail_insert: parking_lot::Mutex<bool>,
}

impl MockHotlEscalationStore {
    fn new() -> Self {
        Self::default()
    }

    fn set_fail_insert(&self, v: bool) {
        *self.fail_insert.lock() = v;
    }

    fn insert_row(&self, row: HotlPendingRow) {
        self.pending.insert(row.escalation_id, row);
    }
}

#[async_trait]
impl HotlEscalationStore for MockHotlEscalationStore {
    async fn insert_pending(
        &self,
        parent: HotlEscalationRow,
        child: HotlPendingRow,
    ) -> RepoResult<Uuid> {
        if *self.fail_insert.lock() {
            return Err(RepoError::InvalidArgument("mock insert failed".into()));
        }
        // Pin child.escalation_id to parent.id (mirroring the PG impl).
        let pinned = HotlPendingRow {
            escalation_id: parent.id,
            ..child
        };
        self.pending.insert(parent.id, pinned);
        Ok(parent.id)
    }

    async fn list_pending_unexpired(&self, now: DateTime<Utc>) -> RepoResult<Vec<HotlPendingRow>> {
        let mut out: Vec<HotlPendingRow> = self
            .pending
            .iter()
            .filter(|kv| kv.value().status == "pending" && kv.value().expires_at > now)
            .map(|kv| kv.value().clone())
            .collect();
        out.sort_by_key(|r| r.created_at);
        Ok(out)
    }

    async fn record_decision(
        &self,
        escalation_id: Uuid,
        verdict: StoreVerdict,
        decided_by: Option<String>,
    ) -> RepoResult<bool> {
        let Some(mut entry) = self.pending.get_mut(&escalation_id) else {
            return Ok(false);
        };
        if entry.status != "pending" {
            return Ok(false);
        }
        entry.status = verdict.status_str().to_string();
        entry.decided_by = decided_by;
        entry.decided_at = Some(Utc::now());
        Ok(true)
    }

    async fn terminalise(&self, escalation_id: Uuid, verdict: StoreVerdict) -> RepoResult<bool> {
        // Mirrors the sqlite impl: stamps any still-`pending` row,
        // REGARDLESS of `expires_at` (that's the timeout-sweep contract).
        let Some(mut entry) = self.pending.get_mut(&escalation_id) else {
            return Ok(false);
        };
        if entry.status != "pending" {
            return Ok(false);
        }
        entry.status = verdict.status_str().to_string();
        entry.decided_at = Some(Utc::now());
        Ok(true)
    }
}

// ── fixture helpers ───────────────────────────────────────────────────────────

fn parent_row(id: Uuid) -> HotlEscalationRow {
    HotlEscalationRow {
        id,
        session_id: Uuid::new_v4(),
        top_level_scope: "tool_call.mcp.test".to_string(),
        status: "pending".to_string(),
        created_at: Utc::now(),
        parent_id: None,
    }
}

fn child_row(escalation_id: Uuid, expires_at: DateTime<Utc>) -> HotlPendingRow {
    HotlPendingRow {
        id: Uuid::new_v4(),
        escalation_id,
        scope: "tool_call.mcp.test".to_string(),
        tool: "mcp.test".to_string(),
        args_redacted: json!({}),
        status: "pending".to_string(),
        expires_at,
        created_at: Utc::now(),
        decided_at: None,
        decided_by: None,
    }
}

// ── 1. replay_reattaches_pending_unexpired ───────────────────────────────────
#[tokio::test]
async fn replay_reattaches_pending_unexpired() {
    let store = Arc::new(MockHotlEscalationStore::new());
    // 5 pending rows, all in the future.
    let future = Utc::now() + chrono::Duration::minutes(5);
    for _ in 0..5 {
        let pid = Uuid::new_v4();
        store.insert_row(child_row(pid, future));
    }

    let registry = DecisionRegistry::replay_from_storage(
        store.clone() as Arc<dyn HotlEscalationStore>,
        Utc::now(),
    )
    .await
    .expect("replay must succeed");

    assert_eq!(
        registry.len(),
        5,
        "all 5 unexpired pending rows must produce in-memory waiters"
    );
}

// ── 2. replay_drops_expired ──────────────────────────────────────────────────
#[tokio::test]
async fn replay_drops_expired() {
    let store = Arc::new(MockHotlEscalationStore::new());
    let past = Utc::now() - chrono::Duration::minutes(5);
    let future = Utc::now() + chrono::Duration::minutes(5);
    store.insert_row(child_row(Uuid::new_v4(), past));
    store.insert_row(child_row(Uuid::new_v4(), future));

    let registry = DecisionRegistry::replay_from_storage(
        store.clone() as Arc<dyn HotlEscalationStore>,
        Utc::now(),
    )
    .await
    .expect("replay must succeed");

    assert_eq!(
        registry.len(),
        1,
        "expired row must be filtered at the SQL boundary"
    );
}

// ── 3. resolve_after_replay_fires_oneshot ────────────────────────────────────
#[tokio::test]
async fn resolve_after_replay_fires_oneshot() {
    // This test requires a way to read back the ticket after replay. The
    // S13-5 design moves ticket ownership into the registry itself; for
    // the unit test we exercise the public API:
    //
    //   1. Replay populates `waiters`.
    //   2. `resolve` looks up the in-memory sender by escalation_id and
    //      delivers the verdict via oneshot.
    //
    // We construct the registry by replaying ONE row whose id we know,
    // then `register_with_ticket` via the new register API to obtain a
    // ticket BEFORE we call resolve (the replay path itself doesn't
    // hand the ticket back — it just installs the sender side; this is
    // why a server restart can wake a *new* operator, but the original
    // loop is gone with the process).
    //
    // For this assertion we instead use `register` + `resolve` to verify
    // the post-replay registry behaves identically to a fresh one.

    let store = Arc::new(MockHotlEscalationStore::new());
    let future = Utc::now() + chrono::Duration::minutes(5);
    let escalation_id = Uuid::new_v4();
    store.insert_row(child_row(escalation_id, future));

    let registry = DecisionRegistry::replay_from_storage(
        store.clone() as Arc<dyn HotlEscalationStore>,
        Utc::now(),
    )
    .await
    .expect("replay must succeed");

    // Resolve persists the verdict, but reports `resumed=false`: replay
    // drops the ticket at mint time (the original loop died with the old
    // process), so the oneshot send fails and nothing actually resumed.
    // (Round-3 "resumed cosmetic" fix — previously this lied `true`.)
    let resolved = registry
        .resolve_persisted(
            escalation_id,
            HotlResolution::Allow,
            Some("ops@acme.com".into()),
        )
        .await
        .expect("resolve must succeed");
    assert!(
        !resolved,
        "post-replay resolve has no live receiver — resumed must be false"
    );
    let snap = store
        .pending
        .get(&escalation_id)
        .expect("row still present in mock")
        .value()
        .clone();
    assert_eq!(
        snap.status, "resolved",
        "the verdict must still be persisted even though no loop resumed"
    );
}

// ── 4. register_persists_then_in_memory (failure-injection) ──────────────────
#[tokio::test]
async fn register_persists_failure_leaves_no_in_memory_waiter() {
    let store = Arc::new(MockHotlEscalationStore::new());
    store.set_fail_insert(true);

    let registry = Arc::new(DecisionRegistry::with_store(
        store.clone() as Arc<dyn HotlEscalationStore>
    ));

    let escalation_id = Uuid::new_v4();
    let parent = parent_row(escalation_id);
    let child = child_row(escalation_id, Utc::now() + chrono::Duration::minutes(5));
    let result = registry
        .register_persisted(
            escalation_id,
            parent,
            child,
            Instant::now() + Duration::from_secs(60),
        )
        .await;
    assert!(matches!(result, Err(RegistryError::Storage(_))));
    assert_eq!(registry.len(), 0, "no in-memory state on persist failure");
}

// ── 5. resolve unknown escalation ────────────────────────────────────────────
#[tokio::test]
async fn resolve_with_no_matching_row_returns_unknown_escalation() {
    let store = Arc::new(MockHotlEscalationStore::new());
    let registry = Arc::new(DecisionRegistry::with_store(
        store.clone() as Arc<dyn HotlEscalationStore>
    ));

    let err = registry
        .resolve_persisted(Uuid::new_v4(), HotlResolution::Allow, None)
        .await
        .expect_err("unknown escalation must be an error");
    assert!(matches!(err, RegistryError::UnknownEscalation));
}

// ── 6. resolve persists then fires the oneshot in that order ─────────────────
#[tokio::test]
async fn resolve_persists_before_firing_oneshot() {
    let store = Arc::new(MockHotlEscalationStore::new());
    let registry = Arc::new(DecisionRegistry::with_store(
        store.clone() as Arc<dyn HotlEscalationStore>
    ));

    let escalation_id = Uuid::new_v4();
    let parent = parent_row(escalation_id);
    let child = child_row(escalation_id, Utc::now() + chrono::Duration::minutes(5));
    let ticket = registry
        .register_persisted(
            escalation_id,
            parent,
            child,
            Instant::now() + Duration::from_secs(60),
        )
        .await
        .expect("register must succeed");

    let resolved = registry
        .resolve_persisted(escalation_id, HotlResolution::Allow, Some("alice".into()))
        .await
        .expect("resolve must succeed");
    assert!(resolved, "live waiter must receive the verdict");

    // PG row must have been stamped first.
    let snap = store
        .pending
        .get(&escalation_id)
        .expect("row still present in mock")
        .value()
        .clone();
    assert_eq!(snap.status, "resolved");
    assert_eq!(snap.decided_by.as_deref(), Some("alice"));

    // And the ticket future resolves with the operator's verdict.
    let cancel = CancellationToken::new();
    let settled = tokio::time::timeout(Duration::from_secs(2), ticket.await_decision(&cancel))
        .await
        .expect("ticket must resolve within timeout")
        .expect("ticket must not error");
    assert_eq!(settled.verdict, HotlResolution::Allow);
    assert_eq!(settled.decided_by.as_deref(), Some("alice"));
}

// ── 7. timeout companion terminalises the store row (audit F1b) ──────────────
#[tokio::test]
async fn register_timeout_companion_terminalises_store_row() {
    let store = Arc::new(MockHotlEscalationStore::new());
    let registry = Arc::new(DecisionRegistry::with_store(
        store.clone() as Arc<dyn HotlEscalationStore>
    ));

    let escalation_id = Uuid::new_v4();
    let parent = parent_row(escalation_id);
    let child = child_row(
        escalation_id,
        Utc::now() + chrono::Duration::milliseconds(50),
    );
    let _ticket = registry
        .register_persisted(
            escalation_id,
            parent,
            child,
            Instant::now() + Duration::from_millis(50),
        )
        .await
        .expect("register must succeed");

    // Give the companion task time to fire + terminalise.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let snap = store
        .pending
        .get(&escalation_id)
        .expect("row must still exist in store")
        .value()
        .clone();
    assert_eq!(
        snap.status, "expired",
        "the timeout companion must terminalise the DB row, not leave it 'pending' forever"
    );
    assert!(
        registry.is_empty(),
        "timeout must also clear the in-memory waiter"
    );
}

// ── 8. boot-replay timeout companion also terminalises ───────────────────────
#[tokio::test]
async fn replay_timeout_companion_terminalises_store_row() {
    let store = Arc::new(MockHotlEscalationStore::new());
    let escalation_id = Uuid::new_v4();
    store.insert_row(child_row(
        escalation_id,
        Utc::now() + chrono::Duration::milliseconds(100),
    ));

    let registry = DecisionRegistry::replay_from_storage(
        store.clone() as Arc<dyn HotlEscalationStore>,
        Utc::now(),
    )
    .await
    .expect("replay must succeed");
    assert_eq!(registry.len(), 1, "row must be reattached by replay");

    tokio::time::sleep(Duration::from_millis(400)).await;

    let snap = store
        .pending
        .get(&escalation_id)
        .expect("row must still exist in store")
        .value()
        .clone();
    assert_eq!(
        snap.status, "expired",
        "the replay companion must terminalise the DB row on expiry"
    );
    assert!(registry.is_empty());
}

// ── 9. resumed cosmetic: dropped receiver ⇒ resumed=false ────────────────────
#[tokio::test]
async fn resolve_persisted_reports_false_when_receiver_dropped() {
    let store = Arc::new(MockHotlEscalationStore::new());
    let registry = Arc::new(DecisionRegistry::with_store(
        store.clone() as Arc<dyn HotlEscalationStore>
    ));

    let escalation_id = Uuid::new_v4();
    let parent = parent_row(escalation_id);
    let child = child_row(escalation_id, Utc::now() + chrono::Duration::minutes(5));
    let ticket = registry
        .register_persisted(
            escalation_id,
            parent,
            child,
            Instant::now() + Duration::from_secs(60),
        )
        .await
        .expect("register must succeed");

    // The agent loop was cancelled — the ticket (receiver half) is gone.
    drop(ticket);

    let resumed = registry
        .resolve_persisted(escalation_id, HotlResolution::Allow, Some("alice".into()))
        .await
        .expect("resolve must succeed");
    assert!(
        !resumed,
        "receiver dropped (cancelled loop) ⇒ resumed must be false, not a cosmetic true"
    );

    // The operator's verdict is still the persisted source of truth.
    let snap = store
        .pending
        .get(&escalation_id)
        .expect("row still present in mock")
        .value()
        .clone();
    assert_eq!(snap.status, "resolved");
    assert_eq!(snap.decided_by.as_deref(), Some("alice"));
}
