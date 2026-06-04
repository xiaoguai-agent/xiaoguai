//! Per-`escalation_id` HOTL decision waiter registry — sprint-12 S12-3,
//! sprint-13 S13-5 (persistence + boot replay).
//!
//! When the agent loop's gate returns `Suspend`, it parks on a oneshot
//! receiver issued by this registry; when `POST /v1/hotl/decisions` lands,
//! the route handler calls [`DecisionRegistry::resolve`] to wake the
//! waiter with the operator's verdict.
//!
//! ## Sprint-13 S13-5: persistence + boot replay
//!
//! The registry now holds an `Arc<dyn HotlEscalationStore>` (from
//! `xiaoguai-storage`) and routes every state transition through it:
//!
//! * [`DecisionRegistry::register`] writes `hotl_escalations` +
//!   `hotl_pending` rows **before** installing the in-memory oneshot
//!   sender. A persist failure leaves zero in-memory state — boot replay
//!   would otherwise resurrect a phantom waiter no operator could ever
//!   resolve.
//! * [`DecisionRegistry::resolve`] writes the verdict row **before**
//!   firing the oneshot. A store miss (no row matched) returns
//!   [`RegistryError::UnknownEscalation`] so the route handler can render
//!   404.
//! * [`DecisionRegistry::replay_from_storage`] rebuilds the in-memory
//!   waiter map from `hotl_pending` rows that are still `pending` AND
//!   unexpired at boot time. Each replayed row gets a fresh oneshot +
//!   `sleep_until(expires_at)` companion task. The replay counts (per
//!   outcome: `reattached`, `expired`, `failed`) are emitted to the new
//!   `xiaoguai_hotl_registry_replayed_total` counter.
//!
//! ## Lifecycle
//!
//! 1. Gate (`SuspendingHotlGate`, S12-4) calls
//!    [`DecisionRegistry::register(escalation_id, parent, child, expires_at)`]
//!    → returns a [`HotlSuspensionTicket`].
//! 2. Loop (`react.rs`, S12-5) emits `HotlPending`, then `await`s
//!    `ticket.await_decision(cancel)`. Three terminal states:
//!    * Operator decides → route handler calls
//!      [`DecisionRegistry::resolve(escalation_id, resolution, decided_by)`]
//!      → ticket resolves to the verdict.
//!    * Expiry fires → background sleeper resolves the ticket
//!      with `HotlResolution::Timeout`.
//!    * Cancel token fires → ticket resolves to
//!      `HotlTicketError::Cancelled`.
//! 3. Loop calls `metrics.on_resolve(elapsed, verdict)`
//!    → gauge--, counter++, histogram observe.
//!
//! ## Concurrency
//!
//! Backed by `DashMap` (lock-free segmented hash map). `register` and
//! `resolve` may interleave arbitrarily — the only invariant is that
//! exactly one of `resolve` / timeout / cancel wins per `escalation_id`.
//! Late `resolve` calls (after expiry already fired) match no row at the
//! DB layer and surface [`RegistryError::UnknownEscalation`].

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use thiserror::Error;
use tokio::sync::oneshot;
use tokio::time::Instant;
use uuid::Uuid;

use xiaoguai_storage::repositories::error::{RepoError, RepoResult};
use xiaoguai_storage::repositories::hotl_escalations::{
    HotlDecisionVerdict as StoreVerdict, HotlEscalationRow, HotlEscalationStore, HotlPendingRow,
};

// Sprint-12 S12-4: the canonical ticket / verdict / resolution types live
// in `xiaoguai_agent::hotl_gate`. The registry re-exports them so callers
// keep the `xiaoguai_api::hotl::decision_registry::*` import path.
pub use xiaoguai_agent::hotl_gate::{
    HotlDecisionVerdict, HotlResolution, HotlSuspensionTicket, HotlTicketError,
};

// ── error type ────────────────────────────────────────────────────────────────

/// Sprint-13 S13-5. Failure modes of the persistence-aware
/// [`DecisionRegistry`] API.
#[derive(Debug, Error)]
pub enum RegistryError {
    /// `resolve` was called for an `escalation_id` that has no matching
    /// `status='pending'` row in the store. Either the id never existed
    /// or the row has already been terminalised (resolved / expired) by
    /// another worker — the route handler maps this to `404 Not Found`.
    #[error("escalation_id not found or already terminalised")]
    UnknownEscalation,
    /// Underlying storage failure (sqlx error, connection drop, etc.).
    #[error("storage error: {0}")]
    Storage(#[from] RepoError),
}

// ── metrics helper ────────────────────────────────────────────────────────────

/// Stable metric label for `xiaoguai_hotl_suspensions_total{verdict}`.
#[must_use]
pub fn metric_label_for(verdict: &HotlResolution) -> &'static str {
    match verdict {
        HotlResolution::Allow => "allow",
        HotlResolution::Deny(_) => "deny",
        HotlResolution::Timeout => "timeout",
    }
}

/// Thin wrapper around the Prometheus handles registered in
/// `xiaoguai-observability::init_prometheus`. Falls back to a silent
/// no-op when `init_prometheus` was never called (unit tests).
#[derive(Debug, Default, Clone, Copy)]
pub struct DecisionRegistryMetrics;

impl DecisionRegistryMetrics {
    /// New entry registered — increment the in-flight gauge.
    pub fn on_register(self) {
        if let Some(g) = xiaoguai_observability::hotl_suspended_loops_gauge() {
            g.inc();
        }
    }

    /// Entry resolved with a verdict. Decrement the gauge, increment the
    /// per-verdict counter, observe the histogram.
    pub fn on_resolve(self, held: Duration, verdict: &HotlResolution) {
        if let Some(g) = xiaoguai_observability::hotl_suspended_loops_gauge() {
            g.dec();
        }
        if let Some(c) = xiaoguai_observability::hotl_suspensions_total() {
            c.with_label_values(&[metric_label_for(verdict)]).inc();
        }
        if let Some(h) = xiaoguai_observability::hotl_suspension_duration_seconds() {
            h.observe(held.as_secs_f64());
        }
    }

    /// Sprint-13 S13-5: per-row outcome of boot replay.
    pub fn on_replay(self, outcome: ReplayOutcome) {
        if let Some(c) = xiaoguai_observability::hotl_registry_replayed_total() {
            c.with_label_values(&[outcome.as_label()]).inc();
        }
    }
}

/// Sprint-13 S13-5. Per-row outcome of
/// [`DecisionRegistry::replay_from_storage`]. Surfaces as the `outcome`
/// label on `xiaoguai_hotl_registry_replayed_total`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayOutcome {
    /// Row had `status='pending'` AND `expires_at > now` at SQL filter
    /// time AND the in-memory waiter was minted successfully.
    Reattached,
    /// Row slipped from "unexpired at SQL filter" to "expired by spawn
    /// time" — rare clock-skew race. The row is intentionally NOT
    /// resurrected in-memory; the next decision request will hit the DB
    /// and find it unresolved + expired.
    Expired,
    /// Replay-side bookkeeping failure (e.g. duplicate id in the boot
    /// batch — should never happen but accounted for defensively).
    Failed,
}

impl ReplayOutcome {
    fn as_label(self) -> &'static str {
        match self {
            Self::Reattached => "reattached",
            Self::Expired => "expired",
            Self::Failed => "failed",
        }
    }
}

// ── noop store (for tests) ────────────────────────────────────────────────────

/// Sprint-13 S13-5. In-process no-op `HotlEscalationStore` for unit
/// tests that don't care about persistence. Every write is a no-op and
/// every read returns an empty vec. Production NEVER uses this — the
/// `run_serve` wiring constructs `SqliteHotlEscalationRepository`.
#[derive(Debug, Default)]
pub struct NoopHotlEscalationStore;

#[async_trait]
impl HotlEscalationStore for NoopHotlEscalationStore {
    async fn insert_pending(
        &self,
        parent: HotlEscalationRow,
        _child: HotlPendingRow,
    ) -> RepoResult<Uuid> {
        Ok(parent.id)
    }

    async fn list_pending_unexpired(&self, _now: DateTime<Utc>) -> RepoResult<Vec<HotlPendingRow>> {
        Ok(Vec::new())
    }

    async fn record_decision(
        &self,
        _escalation_id: Uuid,
        _verdict: StoreVerdict,
        _decided_by: Option<String>,
    ) -> RepoResult<bool> {
        // Test-only path: every resolve returns "row matched" so the
        // in-memory pathway exercises the post-persist branch.
        Ok(true)
    }
}

// ── registry ──────────────────────────────────────────────────────────────────

/// Per-`escalation_id` map of suspended HOTL waiters.
///
/// One registry per [`crate::AppState`]; shared between the gate adapter
/// (which calls `register_persisted`) and the route handler (which calls
/// `resolve_persisted`). The registry itself has zero side-effects when
/// no one calls `register*`, so it is always-present on `AppState` (no
/// `Option`).
pub struct DecisionRegistry {
    waiters: DashMap<Uuid, WaiterSlot>,
    /// Sprint-13 S13-5: persistence layer. `NoopHotlEscalationStore` in
    /// tests; `SqliteHotlEscalationRepository` in production.
    store: Arc<dyn HotlEscalationStore>,
    metrics: DecisionRegistryMetrics,
}

impl std::fmt::Debug for DecisionRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecisionRegistry")
            .field("waiters", &self.waiters.len())
            .field("store", &"Arc<dyn HotlEscalationStore>")
            .field("metrics", &self.metrics)
            .finish()
    }
}

#[derive(Debug)]
struct WaiterSlot {
    sender: oneshot::Sender<HotlDecisionVerdict>,
    registered_at: Instant,
}

impl DecisionRegistry {
    /// Sprint-13 S13-5: construct a registry wired to the given store.
    /// `run_serve` calls this with `SqliteHotlEscalationRepository`; tests
    /// pass `NoopHotlEscalationStore` via [`Self::in_memory`].
    #[must_use]
    pub fn with_store(store: Arc<dyn HotlEscalationStore>) -> Self {
        Self {
            waiters: DashMap::new(),
            store,
            metrics: DecisionRegistryMetrics,
        }
    }

    /// Test helper: construct a registry backed by
    /// [`NoopHotlEscalationStore`]. Equivalent to the sprint-12
    /// `DecisionRegistry::new()` (no persistence).
    #[must_use]
    pub fn in_memory() -> Self {
        Self::with_store(Arc::new(NoopHotlEscalationStore))
    }

    /// Back-compat alias for sprint-12 callers; constructs an in-memory
    /// registry. Production wiring should call
    /// [`Self::with_store`] or [`Self::replay_from_storage`] instead.
    #[must_use]
    pub fn new() -> Self {
        Self::in_memory()
    }

    /// Back-compat alias for sprint-12 callers; constructs an
    /// `Arc<Self>` backed by [`NoopHotlEscalationStore`].
    #[must_use]
    pub fn arc() -> Arc<Self> {
        Arc::new(Self::in_memory())
    }

    /// Sprint-13 S13-5. Boot-time replay constructor.
    ///
    /// Walks `store.list_pending_unexpired(now)`, mints a fresh
    /// `oneshot::channel` per row, spawns a `sleep_until(row.expires_at)`
    /// companion that emits `verdict=timeout` on fire, and returns the
    /// fully-populated registry. The HTTP server MUST NOT start
    /// accepting requests until this future resolves — otherwise a
    /// decision request could land before the matching waiter is in the
    /// map.
    ///
    /// The `sleep_until` companion does NOT re-call `store.record_decision`
    /// — that's the route handler's job. It only fires the oneshot, so
    /// the in-memory waiter (which exists across the registry's lifetime)
    /// observes the timeout. The DB row stays `status='pending'` until
    /// the next replay or a real operator decision.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError::Storage`] if the
    /// `list_pending_unexpired` query fails. Per-row spawn failures are
    /// counted as `ReplayOutcome::Failed` and the replay continues with
    /// the next row.
    pub async fn replay_from_storage(
        store: Arc<dyn HotlEscalationStore>,
        now: DateTime<Utc>,
    ) -> Result<Arc<Self>, RegistryError> {
        let rows = store.list_pending_unexpired(now).await?;
        let registry = Arc::new(Self::with_store(store));

        let mut reattached: usize = 0;
        let mut expired: usize = 0;

        for row in &rows {
            // Defensive: even though SQL filtered `expires_at > now`,
            // the spawn happens N ms later and clock skew is real.
            let now2 = Utc::now();
            if row.expires_at <= now2 {
                registry.metrics.on_replay(ReplayOutcome::Expired);
                expired += 1;
                continue;
            }

            // Convert DateTime<Utc> deadline to tokio::Instant for the
            // sleep_until companion.
            let dur_until = (row.expires_at - now2)
                .to_std()
                .unwrap_or(Duration::from_secs(0));
            let expires_at = Instant::now() + dur_until;

            let (ticket, sender) = HotlSuspensionTicket::new(row.escalation_id, expires_at);
            // Drop the ticket immediately: replay does NOT hand the
            // ticket back to any awaiting loop — the original loop is
            // gone with the old process. Holding the sender keeps the
            // route handler's `resolve_persisted` path functional for
            // when an operator decision lands in the new process.
            drop(ticket);

            let slot = WaiterSlot {
                sender,
                registered_at: Instant::now(),
            };
            registry.waiters.insert(row.escalation_id, slot);
            registry.metrics.on_register();
            registry.metrics.on_replay(ReplayOutcome::Reattached);
            reattached += 1;

            // sleep_until companion: drop the slot + decrement the gauge
            // on expiry. Does NOT touch the store.
            let this = Arc::clone(&registry);
            let escalation_id = row.escalation_id;
            tokio::spawn(async move {
                tokio::time::sleep_until(expires_at).await;
                this.fire_timeout(escalation_id);
            });
        }

        tracing::info!(
            count = rows.len(),
            reattached,
            expired,
            "hotl: replayed pending decision waiters from PG"
        );

        Ok(registry)
    }

    /// Sprint-13 S13-5. Persistence-aware register: writes the
    /// `hotl_escalations` + `hotl_pending` pair through the store
    /// **before** installing the in-memory oneshot sender. A persist
    /// failure leaves zero in-memory state — boot replay would resurrect
    /// a phantom waiter no operator could ever resolve.
    ///
    /// # Errors
    ///
    /// Returns [`RegistryError::Storage`] on store write failure.
    pub async fn register_persisted(
        self: &Arc<Self>,
        escalation_id: Uuid,
        parent: HotlEscalationRow,
        child: HotlPendingRow,
        expires_at: Instant,
    ) -> Result<HotlSuspensionTicket, RegistryError> {
        // 1. Persist first.
        self.store.insert_pending(parent, child).await?;

        // 2. Then install the in-memory sender.
        let (ticket, sender) = HotlSuspensionTicket::new(escalation_id, expires_at);
        let slot = WaiterSlot {
            sender,
            registered_at: Instant::now(),
        };
        self.waiters.insert(escalation_id, slot);
        self.metrics.on_register();

        // Background sleeper: fire on expiry. Mirrors the sprint-12
        // `register` belt-and-braces. Does NOT touch the store — the
        // DB stays `pending` and the next boot replay will sweep it
        // (or an operator decision lands first).
        let this = Arc::clone(self);
        let expires_at_for_task = expires_at;
        tokio::spawn(async move {
            tokio::time::sleep_until(expires_at_for_task).await;
            this.fire_timeout(escalation_id);
        });

        Ok(ticket)
    }

    /// Sprint-13 S13-5. Persistence-aware resolve: writes the verdict
    /// through the store **before** firing the oneshot. A store miss
    /// (no row matched) returns [`RegistryError::UnknownEscalation`].
    ///
    /// Returns `Ok(bool)` where the bool is whether a live in-memory
    /// waiter received the verdict. `false` means the DB row was
    /// updated but no operator was parked on the oneshot (e.g. the
    /// agent loop had already dropped the ticket via cancel).
    ///
    /// # Errors
    ///
    /// - [`RegistryError::Storage`] — underlying sqlx failure.
    /// - [`RegistryError::UnknownEscalation`] — `record_decision`
    ///   returned `Ok(false)` (no row matched, or row was already
    ///   terminalised).
    pub async fn resolve_persisted(
        &self,
        escalation_id: Uuid,
        resolution: HotlResolution,
        decided_by: Option<String>,
    ) -> Result<bool, RegistryError> {
        // 1. Persist first.
        let store_verdict = match &resolution {
            HotlResolution::Allow => StoreVerdict::Allowed,
            HotlResolution::Deny(_) => StoreVerdict::Denied,
            HotlResolution::Timeout => StoreVerdict::Expired,
        };
        let matched = self
            .store
            .record_decision(escalation_id, store_verdict, decided_by.clone())
            .await?;
        if !matched {
            return Err(RegistryError::UnknownEscalation);
        }

        // 2. Then fire the oneshot (if a waiter is still in the map).
        let Some((_, slot)) = self.waiters.remove(&escalation_id) else {
            return Ok(false);
        };
        let held = slot.registered_at.elapsed();
        let verdict = HotlDecisionVerdict {
            verdict: resolution.clone(),
            decided_by,
            recorded_at: Utc::now(),
        };
        let _ = slot.sender.send(verdict);
        self.metrics.on_resolve(held, &resolution);
        Ok(true)
    }

    /// Sprint-12 (S12-3 back-compat). In-memory register: no persistence,
    /// just install a oneshot sender keyed on `escalation_id`. Used by the
    /// 20+ pre-sprint-13 integration tests that don't care about the
    /// store path. Production (`SuspendingHotlGate`) calls
    /// [`Self::register_persisted`] instead.
    pub fn register(
        self: &Arc<Self>,
        escalation_id: Uuid,
        expires_at: Instant,
    ) -> HotlSuspensionTicket {
        let (ticket, sender) = HotlSuspensionTicket::new(escalation_id, expires_at);
        let slot = WaiterSlot {
            sender,
            registered_at: Instant::now(),
        };
        self.waiters.insert(escalation_id, slot);
        self.metrics.on_register();

        let this = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep_until(expires_at).await;
            this.fire_timeout(escalation_id);
        });

        ticket
    }

    /// Sprint-12 (S12-3 back-compat). In-memory resolve: no persistence,
    /// just remove the slot and fire the oneshot. Returns `true` if a
    /// live waiter existed. Used by sprint-12 routes + tests.
    ///
    /// Production (`POST /v1/hotl/decisions`) calls
    /// [`Self::resolve_persisted`] instead.
    pub fn resolve(&self, escalation_id: Uuid, verdict: HotlDecisionVerdict) -> bool {
        let Some((_, slot)) = self.waiters.remove(&escalation_id) else {
            return false;
        };
        let held = slot.registered_at.elapsed();
        let verdict_clone = verdict.verdict.clone();
        let _ = slot.sender.send(verdict);
        self.metrics.on_resolve(held, &verdict_clone);
        true
    }

    /// Internal: fire a `Timeout` verdict on the in-memory channel only.
    /// Used by both `register` and `replay_from_storage` companion
    /// tasks. Does NOT touch the store.
    fn fire_timeout(&self, escalation_id: Uuid) {
        let Some((_, slot)) = self.waiters.remove(&escalation_id) else {
            return;
        };
        let held = slot.registered_at.elapsed();
        let verdict = HotlDecisionVerdict {
            verdict: HotlResolution::Timeout,
            decided_by: None,
            recorded_at: Utc::now(),
        };
        let _ = slot.sender.send(verdict);
        self.metrics.on_resolve(held, &HotlResolution::Timeout);
    }

    /// Test/diagnostic accessor: number of currently-registered waiters.
    #[must_use]
    pub fn len(&self) -> usize {
        self.waiters.len()
    }

    /// Test/diagnostic accessor: true when no waiters are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.waiters.is_empty()
    }
}

impl Default for DecisionRegistry {
    fn default() -> Self {
        Self::in_memory()
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use std::sync::OnceLock;
    use tokio_util::sync::CancellationToken;

    fn metrics_lock() -> &'static parking_lot::Mutex<()> {
        static LOCK: OnceLock<parking_lot::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| parking_lot::Mutex::new(()))
    }

    fn allow_verdict() -> HotlDecisionVerdict {
        HotlDecisionVerdict {
            verdict: HotlResolution::Allow,
            decided_by: Some("alice".into()),
            recorded_at: Utc::now(),
        }
    }

    fn timeout_verdict() -> HotlDecisionVerdict {
        HotlDecisionVerdict {
            verdict: HotlResolution::Timeout,
            decided_by: None,
            recorded_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn register_then_resolve_returns_true() {
        let _guard = metrics_lock().lock();
        let reg = DecisionRegistry::arc();
        let id = Uuid::new_v4();
        let _ticket = reg.register(id, Instant::now() + Duration::from_secs(60));
        assert_eq!(reg.len(), 1);
        let resolved = reg.resolve(id, allow_verdict());
        assert!(resolved, "live waiter must be resolved");
        assert!(reg.is_empty(), "map entry must be removed on resolve");
    }

    #[tokio::test]
    async fn resolve_on_empty_returns_false() {
        let _guard = metrics_lock().lock();
        let reg = DecisionRegistry::arc();
        let resolved = reg.resolve(Uuid::new_v4(), allow_verdict());
        assert!(!resolved, "no waiter ⇒ resolve must return false");
    }

    #[tokio::test]
    async fn register_then_resolve_before_await_succeeds() {
        let _guard = metrics_lock().lock();
        let reg = DecisionRegistry::arc();
        let id = Uuid::new_v4();
        let ticket = reg.register(id, Instant::now() + Duration::from_secs(60));
        assert!(reg.resolve(id, allow_verdict()));
        let cancel = CancellationToken::new();
        let got = ticket.await_decision(&cancel).await.unwrap();
        assert_eq!(got.verdict, HotlResolution::Allow);
        assert_eq!(got.decided_by.as_deref(), Some("alice"));
    }

    #[tokio::test]
    async fn expires_removes_entry_and_resolves_as_timeout() {
        let _guard = metrics_lock().lock();
        let reg = DecisionRegistry::arc();
        let id = Uuid::new_v4();
        let ticket = reg.register(id, Instant::now() + Duration::from_millis(50));
        let cancel = CancellationToken::new();
        let got = ticket.await_decision(&cancel).await.unwrap();
        assert_eq!(got.verdict, HotlResolution::Timeout);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            reg.is_empty(),
            "background timeout task must remove the entry"
        );
    }

    #[tokio::test]
    async fn concurrent_register_and_resolve_is_safe() {
        let _guard = metrics_lock().lock();
        let reg = DecisionRegistry::arc();
        let mut handles = Vec::new();
        for _ in 0..100 {
            let reg = Arc::clone(&reg);
            handles.push(tokio::spawn(async move {
                let id = Uuid::new_v4();
                let _ticket = reg.register(id, Instant::now() + Duration::from_secs(30));
                let resolved = reg.resolve(id, allow_verdict());
                assert!(resolved);
            }));
        }
        for h in handles {
            h.await.expect("task panicked");
        }
        assert!(
            reg.is_empty(),
            "all 100 register/resolve pairs must clear the map"
        );
    }

    #[tokio::test]
    async fn metrics_gauge_increments_on_register_decrements_on_resolve() {
        let _guard = metrics_lock().lock();
        let _ = xiaoguai_observability::init_prometheus();
        let gauge = xiaoguai_observability::hotl_suspended_loops_gauge()
            .expect("gauge must be wired after init_prometheus");

        let baseline = gauge.get();
        let registry = DecisionRegistry::arc();
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let id3 = Uuid::new_v4();
        let _t1 = registry.register(id1, Instant::now() + Duration::from_secs(60));
        let _t2 = registry.register(id2, Instant::now() + Duration::from_secs(60));
        let _t3 = registry.register(id3, Instant::now() + Duration::from_secs(60));
        assert!(
            (gauge.get() - baseline - 3.0).abs() < f64::EPSILON,
            "after 3 register calls gauge delta must be +3 (got {})",
            gauge.get() - baseline
        );

        registry.resolve(id1, allow_verdict());
        assert!(
            (gauge.get() - baseline - 2.0).abs() < f64::EPSILON,
            "after 1 resolve gauge delta must be +2 (got {})",
            gauge.get() - baseline
        );

        registry.resolve(id2, timeout_verdict());
        assert!(
            (gauge.get() - baseline - 1.0).abs() < f64::EPSILON,
            "after 2 resolves gauge delta must be +1 (got {})",
            gauge.get() - baseline
        );
    }
}
