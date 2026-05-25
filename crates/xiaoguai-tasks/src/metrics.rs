//! Prometheus counters for the dispatcher.
//!
//! Emitted metric names cross-reference the wave-3 observability branch
//! (`feat/wave3-prometheus-metrics`). When that branch lands, the registry
//! wiring migrates to `xiaoguai_observability::init_prometheus`; until then
//! the counters register on a caller-supplied (or default) registry.
//!
//! | Metric | Type | Description |
//! |---|---|---|
//! | `kanban_dispatched_total` | Counter | Cards claimed from READY |
//! | `kanban_completed_total` | Counter | Cards moved to DONE |
//! | `kanban_failed_total` | Counter | Individual failed attempts |
//! | `kanban_blocked_total` | Counter | Cards that exhausted retries → BLOCKED |
//! | `kanban_timeout_total` | Counter | Cards cancelled due to timeout |

use prometheus::{register_int_counter_vec_with_registry, IntCounterVec, Registry};

/// All counter handles in one cheap-clone struct.
#[derive(Clone)]
pub struct PoolMetrics {
    /// Cards claimed from READY and handed to a worker.
    pub dispatched: IntCounterVec,
    /// Cards moved to DONE (successful execution).
    pub completed: IntCounterVec,
    /// Individual executor failures (one per attempt, not per card).
    pub failed: IntCounterVec,
    /// Cards that exhausted all retries and landed in BLOCKED.
    pub blocked: IntCounterVec,
    /// Cards cancelled because the executor exceeded its timeout.
    pub timed_out: IntCounterVec,
}

impl PoolMetrics {
    /// Register all counters on `registry`.
    ///
    /// # Errors
    ///
    /// Returns an error if any metric name is already registered (e.g. when two
    /// pools share a registry — use [`PoolMetrics::no_op`] in tests instead).
    pub fn register(registry: &Registry) -> Result<Self, prometheus::Error> {
        let label_names = &["tenant"];

        let dispatched = register_int_counter_vec_with_registry!(
            "kanban_dispatched_total",
            "Cards claimed from READY and dispatched to a worker",
            label_names,
            registry
        )?;
        let completed = register_int_counter_vec_with_registry!(
            "kanban_completed_total",
            "Cards moved to DONE after successful execution",
            label_names,
            registry
        )?;
        let failed = register_int_counter_vec_with_registry!(
            "kanban_failed_total",
            "Individual executor failures (one per attempt)",
            label_names,
            registry
        )?;
        let blocked = register_int_counter_vec_with_registry!(
            "kanban_blocked_total",
            "Cards that exhausted all retries and landed in BLOCKED",
            label_names,
            registry
        )?;
        let timed_out = register_int_counter_vec_with_registry!(
            "kanban_timeout_total",
            "Cards cancelled because the executor exceeded its timeout",
            label_names,
            registry
        )?;

        Ok(Self {
            dispatched,
            completed,
            failed,
            blocked,
            timed_out,
        })
    }

    /// No-op metrics (for tests that don't need a registry).
    ///
    /// Uses a throw-away private registry so registration never conflicts.
    #[must_use]
    pub fn no_op() -> Self {
        let r = Registry::new();
        Self::register(&r).expect("fresh registry cannot conflict")
    }

    /// Increment `dispatched` for the given tenant (or `"system"`).
    pub fn inc_dispatched(&self, tenant: &str) {
        self.dispatched.with_label_values(&[tenant]).inc();
    }

    pub fn inc_completed(&self, tenant: &str) {
        self.completed.with_label_values(&[tenant]).inc();
    }

    pub fn inc_failed(&self, tenant: &str) {
        self.failed.with_label_values(&[tenant]).inc();
    }

    pub fn inc_blocked(&self, tenant: &str) {
        self.blocked.with_label_values(&[tenant]).inc();
    }

    pub fn inc_timed_out(&self, tenant: &str) {
        self.timed_out.with_label_values(&[tenant]).inc();
    }
}
