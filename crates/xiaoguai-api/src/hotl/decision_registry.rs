//! Per-`request_id` HOTL decision waiter registry — sprint-12 S12-3.
//!
//! When the agent loop's gate returns `Suspend`, it parks on a oneshot
//! receiver issued by this registry; when `POST /v1/hotl/decisions` lands,
//! the route handler calls [`DecisionRegistry::resolve`] to wake the
//! waiter with the operator's verdict.
//!
//! ## Lifecycle
//!
//! 1. Gate (`SuspendingHotlGate`, S12-4) calls
//!    [`DecisionRegistry::register(request_id, expires_at)`] →
//!    returns a [`HotlSuspensionTicket`].
//! 2. Loop (`react.rs`, S12-5) emits `HotlPending`, then `await`s
//!    `ticket.await_decision(cancel)`. Three terminal states:
//!    * Operator decides → route handler calls
//!      [`DecisionRegistry::resolve(request_id, verdict)`] →
//!      ticket resolves to the verdict.
//!    * Expiry fires → background sleeper resolves the ticket
//!      with `HotlResolution::Timeout`.
//!    * Cancel token fires → ticket resolves to
//!      `HotlTicketError::Cancelled`; loop still calls
//!      `metrics.on_resolve(elapsed, Cancelled)` so the gauge
//!      decrements deterministically.
//! 3. Loop calls `metrics.on_resolve(elapsed, verdict)`
//!    → gauge--, counter++, histogram observe.
//!
//! ## Sprint-12 cross-crate contract note (resolved in S12-4)
//!
//! The canonical home for [`HotlSuspensionTicket`],
//! [`HotlDecisionVerdict`], and [`HotlResolution`] is
//! `xiaoguai_agent::hotl_gate` (sprint-12 S12-1). The S12-3 wave landed
//! local duplicates because S12-1 was an independent parallel PR. S12-4
//! flips the registry over to the agent-crate types so the gate
//! (`SuspendingHotlGate` in `xiaoguai-core::hotl_bridge`) can embed the
//! ticket inside `HotlGateVerdict::Suspend` without a second adapter
//! layer. The wire shape is fixed by the design (`lld-agent.md` §4.5).
//!
//! ## Concurrency
//!
//! Backed by `DashMap` (lock-free segmented hash map). `register` and
//! `resolve` may interleave arbitrarily — the only invariant is that
//! exactly one of `resolve` / timeout / cancel wins per `request_id`.
//! Late `resolve` calls (after expiry already fired) return `false`
//! and are no-ops.
//!
//! ## Persistence
//!
//! In-memory only. Restart drops live waiters; loops blocked at
//! `ticket.await_decision` will be torn down with the process. Per
//! plan §4 out-of-scope: Redis-backed persistence is sprint-14+.

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::oneshot;
use tokio::time::Instant;
use uuid::Uuid;

// Sprint-12 S12-4: the canonical ticket / verdict / resolution types live
// in `xiaoguai_agent::hotl_gate`. The registry re-exports them so callers
// (the route handler, gate adapter, tests) can keep the
// `xiaoguai_api::hotl::decision_registry::*` import path they were already
// using; the underlying types now match what
// `xiaoguai_agent::HotlGateVerdict::Suspend.ticket` carries.
pub use xiaoguai_agent::hotl_gate::{
    HotlDecisionVerdict, HotlResolution, HotlSuspensionTicket, HotlTicketError,
};

// ── metrics helper ────────────────────────────────────────────────────────────

/// Stable metric label for `xiaoguai_hotl_suspensions_total{verdict}`.
/// Free function (not a method on the agent-crate `HotlResolution`) so the
/// label vocabulary stays owned by the registry — sprint-12 S12-5 will
/// extend it with an explicit `"cancelled"` label when the loop tears down
/// a waiter due to session cancel (no resolved verdict was produced).
#[must_use]
pub fn metric_label_for(verdict: &HotlResolution) -> &'static str {
    match verdict {
        HotlResolution::Allow => "allow",
        HotlResolution::Deny(_) => "deny",
        HotlResolution::Timeout => "timeout",
    }
}

/// Thin wrapper around the three Prometheus handles registered in
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

    /// Entry resolved with a verdict from the channel (operator decision
    /// or background-task Timeout). Decrement the gauge, increment the
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
}

// ── registry ──────────────────────────────────────────────────────────────────

/// Per-`request_id` map of suspended HOTL waiters.
///
/// One registry per [`crate::AppState`]; shared between the gate adapter
/// (which calls `register`) and the route handler (which calls
/// `resolve`). The registry itself has zero side-effects when no one
/// calls `register`, so it is always-present on `AppState` (no `Option`).
#[derive(Debug)]
pub struct DecisionRegistry {
    waiters: DashMap<Uuid, WaiterSlot>,
    metrics: DecisionRegistryMetrics,
}

#[derive(Debug)]
struct WaiterSlot {
    sender: oneshot::Sender<HotlDecisionVerdict>,
    registered_at: Instant,
}

impl DecisionRegistry {
    /// Construct an empty registry. Metrics are bound to the global
    /// `xiaoguai-observability` handles lazily; tests that bypass
    /// `init_prometheus` get a silent no-op.
    #[must_use]
    pub fn new() -> Self {
        Self {
            waiters: DashMap::new(),
            metrics: DecisionRegistryMetrics,
        }
    }

    /// Convenience constructor: `Arc::new(Self::new())`.
    #[must_use]
    pub fn arc() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Register a new suspended request. Returns the agent-crate ticket
    /// the loop `await`s; spawns a background sleeper that resolves the
    /// ticket with `HotlResolution::Timeout` on `expires_at`.
    ///
    /// Sprint-12 S12-4: the ticket is built via
    /// `xiaoguai_agent::HotlSuspensionTicket::new` so the registry, the
    /// gate adapter (`SuspendingHotlGate`), and the ReAct loop all share
    /// one type. The registry owns the matching `oneshot::Sender`.
    pub fn register(
        self: &Arc<Self>,
        request_id: Uuid,
        expires_at: Instant,
    ) -> HotlSuspensionTicket {
        let (ticket, sender) = HotlSuspensionTicket::new(request_id, expires_at);
        let slot = WaiterSlot {
            sender,
            registered_at: Instant::now(),
        };
        // If a prior `register` for the same id is still in flight
        // (caller bug — operator UI would never emit duplicate
        // request_ids), drop the old sender so its ticket resolves as
        // `ChannelDropped`.
        self.waiters.insert(request_id, slot);
        self.metrics.on_register();

        // Background sleeper: fire `resolve(.., Timeout)` on expiry. The
        // self.clone() bumps the Arc refcount so the spawned task can
        // outlive the caller scope. The agent ticket's own select! also
        // races `expires_at`, so this background task is belt-and-braces:
        // it ensures the registry map entry is removed and the gauge
        // decremented even if no one awaits the ticket.
        let this = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep_until(expires_at).await;
            // `resolve` is a no-op if the operator already decided; the
            // sender slot is gone in that case.
            let _ = this.resolve(
                request_id,
                HotlDecisionVerdict {
                    verdict: HotlResolution::Timeout,
                    decided_by: None,
                    recorded_at: chrono::Utc::now(),
                },
            );
        });

        ticket
    }

    /// Deliver the operator's verdict (or a Timeout from the background
    /// sleeper) to the parked loop. Returns `true` if a live waiter
    /// existed; `false` if it had already been resolved or never registered.
    pub fn resolve(&self, request_id: Uuid, verdict: HotlDecisionVerdict) -> bool {
        let Some((_, slot)) = self.waiters.remove(&request_id) else {
            return false;
        };
        let held = slot.registered_at.elapsed();
        let verdict_clone = verdict.verdict.clone();
        // `send` only fails if the receiver was dropped. Treat as a
        // successful resolve — the loop tore down its ticket
        // independently (e.g. session cancel beat us to it).
        let _ = slot.sender.send(verdict);
        self.metrics.on_resolve(held, &verdict_clone);
        true
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
        Self::new()
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
// `metrics_lock()` is a `std::sync::Mutex` held across `.await` in these
// tests purely to serialise the process-wide Prometheus gauge — the critical
// sections never block on async work, so the `await_holding_lock` lint is
// a false positive here. Allowed at module scope to avoid sprinkling
// `#[allow(...)]` on every async test.
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use std::sync::OnceLock;
    use tokio_util::sync::CancellationToken;

    /// Global serialisation lock for any test in this module that
    /// observes the Prometheus gauge (which is a process-wide singleton
    /// owned by `xiaoguai-observability`). Tests that only assert local
    /// invariants on the per-test `DecisionRegistry` instance don't
    /// strictly need it, but they DO bump the gauge as a side effect,
    /// so they acquire the lock too to keep the gauge-observation test
    /// race-free. The lock is held for the entire test body — these
    /// tests run in milliseconds, so contention is irrelevant.
    fn metrics_lock() -> &'static parking_lot::Mutex<()> {
        static LOCK: OnceLock<parking_lot::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| parking_lot::Mutex::new(()))
    }

    fn allow_verdict() -> HotlDecisionVerdict {
        HotlDecisionVerdict {
            verdict: HotlResolution::Allow,
            decided_by: Some("alice".into()),
            recorded_at: chrono::Utc::now(),
        }
    }

    fn timeout_verdict() -> HotlDecisionVerdict {
        HotlDecisionVerdict {
            verdict: HotlResolution::Timeout,
            decided_by: None,
            recorded_at: chrono::Utc::now(),
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
        // Race case from §3 risk row 2: resolve lands before the loop
        // even reaches `ticket.await_decision`. Oneshot channel must
        // still deliver the verdict.
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
        // Give the spawned sleeper a chance to remove the entry.
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
        // Serialise against the other gauge-bumping tests in this
        // module — the gauge is a process-wide singleton.
        let _guard = metrics_lock().lock();
        // `init_prometheus` is idempotent w.r.t. the global `OnceCell` —
        // the first call wins, subsequent calls in the same binary
        // silently return a fresh-but-unused registry. We only care
        // about the *delta* on the global handle, not the registry
        // identity, so we ignore the error path.
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

        // Forcibly expire id2 by resolving via the same path the
        // background sleeper would use.
        registry.resolve(id2, timeout_verdict());
        assert!(
            (gauge.get() - baseline - 1.0).abs() < f64::EPSILON,
            "after 2 resolves gauge delta must be +1 (got {})",
            gauge.get() - baseline
        );
    }
}
