//! Bounded-cardinality guard for free-form Prometheus label values.
//!
//! Metrics labelled by `(provider, model)` take user-configured strings:
//! every distinct value mints a new time series, so an unbounded stream of
//! model names (typos, per-request overrides, dynamic suffixes) can explode
//! series cardinality on the scrape side.
//!
//! The guard is a process-wide allow-through set per label namespace: the
//! first [`LABEL_CARDINALITY_CAP`] distinct values pass through verbatim;
//! once the cap is hit, every *new* value maps to [`OTHER_LABEL`].
//! Previously-admitted values keep passing through. A `warn` is logged
//! once per namespace when the cap is first exceeded.
//!
//! Dependency-free by design (std `OnceLock` + `Mutex` only) so it can sit
//! in this crate without pulling anything into the macro call sites.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// Maximum number of distinct values admitted per label namespace.
pub const LABEL_CARDINALITY_CAP: usize = 50;

/// Catch-all label value once the cap is reached.
pub const OTHER_LABEL: &str = "_other";

/// A bounded set of admitted label values for one label namespace.
///
/// `const`-constructible so it can back a `static`; the inner `HashSet`
/// is lazily created on first use via `OnceLock`.
struct BoundedLabelSet {
    /// Label namespace name, used in the one-shot warn log.
    name: &'static str,
    /// Admitted values. Lazily initialised; never shrinks.
    seen: OnceLock<Mutex<HashSet<String>>>,
    /// Whether the cap-hit warn has been emitted already.
    warned: AtomicBool,
}

impl BoundedLabelSet {
    const fn new(name: &'static str) -> Self {
        Self {
            name,
            seen: OnceLock::new(),
            warned: AtomicBool::new(false),
        }
    }

    /// Admit `value` verbatim while under the cap; map new values to
    /// [`OTHER_LABEL`] afterwards. Values admitted before the cap keep
    /// passing through.
    fn normalize<'a>(&self, value: &'a str) -> &'a str {
        let seen = self.seen.get_or_init(|| Mutex::new(HashSet::new()));
        // A poisoned lock only means another thread panicked mid-insert;
        // the set is still usable for a label decision.
        let mut guard = match seen.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.contains(value) {
            return value;
        }
        if guard.len() < LABEL_CARDINALITY_CAP {
            guard.insert(value.to_owned());
            return value;
        }
        drop(guard);
        if !self.warned.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                label = self.name,
                cap = LABEL_CARDINALITY_CAP,
                rejected_value = value,
                "metric label cardinality cap hit; new values now map to \"_other\""
            );
        }
        OTHER_LABEL
    }
}

/// Process-wide admitted `model` label values.
static MODEL_LABELS: BoundedLabelSet = BoundedLabelSet::new("model");
/// Process-wide admitted `provider` label values.
static PROVIDER_LABELS: BoundedLabelSet = BoundedLabelSet::new("provider");

/// Bound the `model` metric label: first [`LABEL_CARDINALITY_CAP`] distinct
/// values pass through, later new values become [`OTHER_LABEL`].
///
/// Use this on every site where a free-form model string becomes a
/// Prometheus label value. Tracing span fields should keep the verbatim
/// value — only the metric label needs bounding.
pub fn bounded_model_label(model: &str) -> &str {
    MODEL_LABELS.normalize(model)
}

/// Bound the `provider` metric label. Same semantics as
/// [`bounded_model_label`] with an independent namespace, so provider churn
/// cannot evict admitted model values (and vice versa).
pub fn bounded_provider_label(provider: &str) -> &str {
    PROVIDER_LABELS.normalize(provider)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    /// Tests run against private per-instance sets, NOT the process-wide
    /// statics — unit tests in this binary run in parallel and must not
    /// poison each other's cap state. The statics are exercised by the
    /// `tests/cardinality_guard.rs` integration binary (own process).
    fn fresh(name: &'static str) -> BoundedLabelSet {
        BoundedLabelSet::new(name)
    }

    #[test]
    fn cardinality_under_cap_passes_values_through() {
        let set = fresh("model");
        for i in 0..LABEL_CARDINALITY_CAP {
            let value = format!("model-{i}");
            assert_eq!(
                set.normalize(&value),
                value,
                "value {i} under cap must pass through verbatim"
            );
        }
    }

    #[test]
    fn cardinality_over_cap_maps_new_values_to_other() {
        let set = fresh("model");
        for i in 0..LABEL_CARDINALITY_CAP {
            set.normalize(&format!("model-{i}"));
        }
        assert_eq!(
            set.normalize("model-overflow"),
            OTHER_LABEL,
            "first value past the cap must map to _other"
        );
        assert_eq!(
            set.normalize("model-overflow-2"),
            OTHER_LABEL,
            "every later new value must map to _other"
        );
    }

    #[test]
    fn cardinality_admitted_values_survive_the_cap() {
        let set = fresh("model");
        for i in 0..LABEL_CARDINALITY_CAP {
            set.normalize(&format!("model-{i}"));
        }
        set.normalize("model-overflow"); // trips the cap
        assert_eq!(
            set.normalize("model-0"),
            "model-0",
            "values admitted before the cap must keep passing through"
        );
    }

    #[test]
    fn cardinality_warns_exactly_once_on_cap_hit() {
        let set = fresh("model");
        for i in 0..LABEL_CARDINALITY_CAP {
            set.normalize(&format!("model-{i}"));
        }
        assert!(
            !set.warned.load(Ordering::Relaxed),
            "no warn before the cap is exceeded"
        );
        set.normalize("model-overflow");
        assert!(
            set.warned.load(Ordering::Relaxed),
            "warn flag set on first over-cap value"
        );
        // Second over-cap value: swap() already returned true once, so the
        // warn branch is unreachable — flag stays set, no second log.
        set.normalize("model-overflow-2");
        assert!(set.warned.load(Ordering::Relaxed));
    }

    #[test]
    fn cardinality_thread_safety_smoke() {
        let set = Arc::new(fresh("model"));
        let handles: Vec<_> = (0..8)
            .map(|t| {
                let set = Arc::clone(&set);
                std::thread::spawn(move || {
                    for i in 0..100 {
                        let value = format!("model-{t}-{i}");
                        let out = set.normalize(&value);
                        assert!(
                            out == value || out == OTHER_LABEL,
                            "normalize must return the value or _other, got {out}"
                        );
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("worker thread panicked");
        }
        let seen = set
            .seen
            .get()
            .expect("set initialised")
            .lock()
            .expect("lock");
        assert!(
            seen.len() <= LABEL_CARDINALITY_CAP,
            "admitted set must never exceed the cap, got {}",
            seen.len()
        );
    }
}
