/// Named detector registry.
///
/// `AnomalyRegistry` owns one `BoxedDetector` per spec ID.  It is the
/// entry-point the scheduler calls during each poll cycle.
///
/// Persistence is abstracted behind the `AnomalyStore` trait so that the
/// in-memory implementation can be used in tests while a future
/// `PgAnomalyStore` implementation wires up PostgreSQL.
use std::collections::HashMap;

use chrono::{DateTime, Utc};
use tracing::{debug, info, warn};

use crate::{
    detector::{Anomaly, Detector, EwmaDetector, ZScoreDetector},
    spec::{AnomalySpec, DetectorKind},
};

// ── Store trait ────────────────────────────────────────────────────────────

/// Persistence interface for recorded anomalies.
pub trait AnomalyStore: Send + Sync {
    /// Persist one anomaly event.
    fn record(&mut self, spec_id: &str, anomaly: &Anomaly);
    /// Return all recorded anomalies (for testing / admin).
    fn all(&self) -> Vec<(String, Anomaly)>;
}

// ── In-memory store ────────────────────────────────────────────────────────

/// Simple in-memory store suitable for tests and single-process deploys.
#[derive(Debug, Default)]
pub struct InMemoryStore {
    events: Vec<(String, Anomaly)>,
}

impl AnomalyStore for InMemoryStore {
    fn record(&mut self, spec_id: &str, anomaly: &Anomaly) {
        self.events.push((spec_id.to_string(), anomaly.clone()));
    }

    fn all(&self) -> Vec<(String, Anomaly)> {
        self.events.clone()
    }
}

// ── BoxedDetector ──────────────────────────────────────────────────────────

type BoxedDetector = Box<dyn Detector>;

fn build_detector(spec: &AnomalySpec) -> BoxedDetector {
    match &spec.detector {
        DetectorKind::ZScore {
            sigma_threshold,
            min_count,
        } => Box::new(ZScoreDetector::new(
            *sigma_threshold,
            *min_count,
            spec.cool_off,
        )),
        DetectorKind::Ewma {
            alpha,
            sigma_threshold,
            min_count,
        } => Box::new(EwmaDetector::new(
            *alpha,
            *sigma_threshold,
            *min_count,
            spec.cool_off,
        )),
    }
}

// ── AnomalyRegistry ────────────────────────────────────────────────────────

/// Central registry that maps spec IDs to live detector instances.
///
/// # Usage (scheduler integration)
///
/// ```rust,ignore
/// let mut registry = AnomalyRegistry::new(Box::new(InMemoryStore::default()));
/// registry.register(spec);
///
/// // In each poll tick:
/// if let Some(anomaly) = registry.observe("orders_anomaly", ts, value) {
///     // dispatch ActionRef from the spec
/// }
/// ```
pub struct AnomalyRegistry {
    specs: HashMap<String, AnomalySpec>,
    detectors: HashMap<String, BoxedDetector>,
    store: Box<dyn AnomalyStore>,
}

impl AnomalyRegistry {
    /// Create a new registry backed by the given store.
    pub fn new(store: Box<dyn AnomalyStore>) -> Self {
        Self {
            specs: HashMap::new(),
            detectors: HashMap::new(),
            store,
        }
    }

    /// Register a spec, creating (or replacing) its detector.
    pub fn register(&mut self, spec: AnomalySpec) {
        let id = spec.id.clone();
        info!(id, "Registering anomaly spec");
        let detector = build_detector(&spec);
        self.specs.insert(id.clone(), spec);
        self.detectors.insert(id, detector);
    }

    /// Remove a spec and its detector.
    pub fn deregister(&mut self, id: &str) {
        self.specs.remove(id);
        self.detectors.remove(id);
        info!(id, "Deregistered anomaly spec");
    }

    /// Feed one observation into the named detector.
    ///
    /// Returns `Some((Anomaly, &AnomalySpec))` when an alert fires so the
    /// caller can dispatch the `on_anomaly` action.
    pub fn observe(
        &mut self,
        id: &str,
        ts: DateTime<Utc>,
        value: f64,
    ) -> Option<(&AnomalySpec, Anomaly)> {
        let Some(detector) = self.detectors.get_mut(id) else {
            warn!(id, "observe() called for unknown spec");
            return None;
        };

        let anomaly = detector.observe(ts, value)?;

        // Persist the event.
        self.store.record(id, &anomaly);
        debug!(id, score = anomaly.score, "Anomaly stored");

        // Return a ref to the spec so the caller can read `on_anomaly`.
        let spec = self
            .specs
            .get(id)
            .expect("spec always present if detector is");
        Some((spec, anomaly))
    }

    /// All recorded anomalies (from the backing store).
    pub fn recorded_anomalies(&self) -> Vec<(String, Anomaly)> {
        self.store.all()
    }

    /// Names of all registered specs.
    pub fn registered_ids(&self) -> Vec<&str> {
        self.specs.keys().map(String::as_str).collect()
    }

    /// Look up a spec by ID (read-only).
    pub fn spec(&self, id: &str) -> Option<&AnomalySpec> {
        self.specs.get(id)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{ActionRef, DetectorKind};
    use chrono::{Duration, TimeZone};

    fn t(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    fn make_spec(id: &str) -> AnomalySpec {
        AnomalySpec {
            id: id.to_string(),
            kpi_query: "SELECT 1".to_string(),
            window: Duration::minutes(60),
            detector: DetectorKind::ZScore {
                sigma_threshold: 3.0,
                min_count: 5,
            },
            cool_off: Duration::seconds(30),
            on_anomaly: ActionRef::Notify {
                channel: "test".to_string(),
            },
        }
    }

    #[test]
    fn register_and_observe_no_anomaly() {
        let mut reg = AnomalyRegistry::new(Box::new(InMemoryStore::default()));
        reg.register(make_spec("test1"));
        for i in 0i64..20 {
            let r = reg.observe("test1", t(i), 42.0);
            assert!(r.is_none());
        }
        assert!(reg.recorded_anomalies().is_empty());
    }

    #[test]
    fn observe_unknown_id_returns_none() {
        let mut reg = AnomalyRegistry::new(Box::new(InMemoryStore::default()));
        let r = reg.observe("ghost", t(0), 1.0);
        assert!(r.is_none());
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn spike_recorded_in_store() {
        let mut reg = AnomalyRegistry::new(Box::new(InMemoryStore::default()));
        reg.register(make_spec("spike_test"));
        // Stable baseline
        for i in 0i64..20 {
            reg.observe("spike_test", t(i), 10.0 + (i % 2) as f64 * 0.1);
        }
        // Spike
        let result = reg.observe("spike_test", t(20), 9999.0);
        assert!(result.is_some(), "spike should fire");
        let events = reg.recorded_anomalies();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "spike_test");
    }

    #[test]
    fn deregister_removes_spec() {
        let mut reg = AnomalyRegistry::new(Box::new(InMemoryStore::default()));
        reg.register(make_spec("to_remove"));
        assert!(reg.spec("to_remove").is_some());
        reg.deregister("to_remove");
        assert!(reg.spec("to_remove").is_none());
    }
}
