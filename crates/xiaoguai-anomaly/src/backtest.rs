//! Offline back-testing: replay a fixed `(timestamp, value)` series through a
//! detector built from an [`AnomalySpec`], collecting every [`Anomaly`] it
//! fires.
//!
//! Pure and deterministic — no I/O, no data source, no clock. This is the
//! engine behind the `POST /v1/anomaly/test` REST endpoint and the
//! `xiaoguai anomaly test` CLI: it lets an operator evaluate a spec against
//! historical CSV data before wiring it into the live scheduler.
//!
//! Detector parameters are validated here (at the boundary) so malformed
//! input surfaces as a typed [`BacktestError`] rather than a panic from
//! [`EwmaDetector::new`]'s `alpha` assertion.

use chrono::{DateTime, Duration, Utc};
use thiserror::Error;

use crate::detector::{Anomaly, Detector, EwmaDetector, ZScoreDetector};
use crate::spec::{AnomalySpec, DetectorKind};

/// Why a back-test could not run. The only failure mode is a structurally
/// invalid detector configuration; bad data points are tolerated (a
/// non-finite observation simply never scores as an anomaly).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum BacktestError {
    /// A detector parameter is outside its valid range (e.g. `alpha ∉ [0, 1]`
    /// or a non-positive `sigma_threshold`).
    #[error("invalid detector config: {0}")]
    InvalidDetector(String),
}

/// Validate `sigma_threshold` — shared by both detector kinds.
fn check_sigma(sigma_threshold: f64) -> Result<(), BacktestError> {
    if sigma_threshold.is_finite() && sigma_threshold > 0.0 {
        Ok(())
    } else {
        Err(BacktestError::InvalidDetector(format!(
            "sigma_threshold must be a positive finite number, got {sigma_threshold}"
        )))
    }
}

/// Construct a fresh, unarmed detector from a declarative [`DetectorKind`] and
/// cooldown. Mirrors the scheduler's runtime wiring so back-test results match
/// live behaviour.
///
/// # Errors
/// Returns [`BacktestError::InvalidDetector`] when a parameter is out of range
/// (`alpha ∉ [0, 1]`, or a non-positive / non-finite `sigma_threshold`).
pub fn build_detector(
    kind: &DetectorKind,
    cool_off: Duration,
) -> Result<Box<dyn Detector>, BacktestError> {
    match *kind {
        DetectorKind::ZScore {
            sigma_threshold,
            min_count,
        } => {
            check_sigma(sigma_threshold)?;
            Ok(Box::new(ZScoreDetector::new(
                sigma_threshold,
                min_count,
                cool_off,
            )))
        }
        DetectorKind::Ewma {
            alpha,
            sigma_threshold,
            min_count,
        } => {
            if !(0.0..=1.0).contains(&alpha) {
                return Err(BacktestError::InvalidDetector(format!(
                    "alpha must be in [0, 1], got {alpha}"
                )));
            }
            check_sigma(sigma_threshold)?;
            Ok(Box::new(EwmaDetector::new(
                alpha,
                sigma_threshold,
                min_count,
                cool_off,
            )))
        }
    }
}

/// Replay `points` (in the given order) through a detector built from `spec`,
/// returning every anomaly that fires.
///
/// The detectors are online and order-sensitive, exactly as in live polling,
/// so the caller should pass points sorted by timestamp; out-of-order input is
/// fed verbatim. Non-finite values are passed through and simply never fire.
///
/// # Errors
/// Propagates [`BacktestError`] from [`build_detector`] when the spec's
/// detector parameters are invalid.
pub fn backtest(
    spec: &AnomalySpec,
    points: &[(DateTime<Utc>, f64)],
) -> Result<Vec<Anomaly>, BacktestError> {
    let mut detector = build_detector(&spec.detector, spec.cool_off)?;
    Ok(points
        .iter()
        .filter_map(|&(ts, value)| detector.observe(ts, value))
        .collect())
}

// ── Unit tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::ActionRef;
    use chrono::TimeZone;

    fn t(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    /// A complete spec with the given detector and a 0-second cooldown so every
    /// qualifying spike can fire during a back-test.
    fn spec_with(detector: DetectorKind) -> AnomalySpec {
        AnomalySpec {
            id: "test".to_string(),
            kpi_query: "n/a".to_string(),
            window: Duration::hours(1),
            detector,
            cool_off: Duration::seconds(0),
            on_anomaly: ActionRef::Notify {
                channel: "ops".to_string(),
            },
            schedule: crate::spec::AnomalySchedule::default(),
        }
    }

    /// Build `(t(i), baseline±1)` for i in 0..n so σ > 0 once armed.
    fn flat_series(n: i64, baseline: f64) -> Vec<(DateTime<Utc>, f64)> {
        (0..n)
            .map(|i| (t(i), baseline + if i % 2 == 0 { -1.0 } else { 1.0 }))
            .collect()
    }

    #[test]
    fn backtest_flags_injected_spike() {
        let spec = spec_with(DetectorKind::ZScore {
            sigma_threshold: 3.0,
            min_count: 10,
        });
        let mut points = flat_series(30, 100.0);
        points.push((t(30), 5000.0)); // clear spike

        let anomalies = backtest(&spec, &points).expect("valid spec");
        assert_eq!(anomalies.len(), 1, "exactly one spike should fire");
        assert_eq!(anomalies[0].ts, t(30));
        assert!(anomalies[0].score > 3.0);
    }

    #[test]
    fn backtest_constant_series_no_anomaly() {
        let spec = spec_with(DetectorKind::ZScore {
            sigma_threshold: 3.0,
            min_count: 5,
        });
        let points: Vec<_> = (0..50).map(|i| (t(i), 100.0)).collect();
        let anomalies = backtest(&spec, &points).expect("valid spec");
        assert!(anomalies.is_empty(), "constant series must never fire");
    }

    #[test]
    fn backtest_empty_input_is_empty() {
        let spec = spec_with(DetectorKind::default());
        let anomalies = backtest(&spec, &[]).expect("valid spec");
        assert!(anomalies.is_empty());
    }

    #[test]
    fn backtest_respects_cooldown() {
        // Cooldown of 60 s: two spikes 1 s apart → only the first fires.
        let mut spec = spec_with(DetectorKind::ZScore {
            sigma_threshold: 3.0,
            min_count: 5,
        });
        spec.cool_off = Duration::seconds(60);
        let mut points = flat_series(20, 10.0);
        points.push((t(20), 100_000.0));
        points.push((t(21), 100_000.0)); // within cooldown → suppressed

        let anomalies = backtest(&spec, &points).expect("valid spec");
        assert_eq!(anomalies.len(), 1, "cooldown should suppress the repeat");
        assert_eq!(anomalies[0].ts, t(20));
    }

    #[test]
    fn backtest_ewma_detector_runs() {
        let spec = spec_with(DetectorKind::Ewma {
            alpha: 0.2,
            sigma_threshold: 3.0,
            min_count: 10,
        });
        let mut points = flat_series(30, 50.0);
        points.push((t(30), 5000.0));
        let anomalies = backtest(&spec, &points).expect("valid spec");
        assert_eq!(anomalies.len(), 1);
    }

    #[test]
    fn backtest_rejects_alpha_out_of_range() {
        let spec = spec_with(DetectorKind::Ewma {
            alpha: 5.0, // invalid
            sigma_threshold: 3.0,
            min_count: 10,
        });
        let err = backtest(&spec, &[]).unwrap_err();
        assert!(matches!(err, BacktestError::InvalidDetector(_)));
    }

    #[test]
    fn backtest_rejects_nonpositive_sigma() {
        let spec = spec_with(DetectorKind::ZScore {
            sigma_threshold: 0.0, // invalid
            min_count: 5,
        });
        let err = backtest(&spec, &[]).unwrap_err();
        assert_eq!(
            err,
            BacktestError::InvalidDetector(
                "sigma_threshold must be a positive finite number, got 0".to_string()
            )
        );
    }

    #[test]
    fn build_detector_both_kinds_ok() {
        assert!(build_detector(
            &DetectorKind::ZScore {
                sigma_threshold: 3.0,
                min_count: 5
            },
            Duration::seconds(0)
        )
        .is_ok());
        assert!(build_detector(
            &DetectorKind::Ewma {
                alpha: 0.1,
                sigma_threshold: 3.0,
                min_count: 5
            },
            Duration::seconds(0)
        )
        .is_ok());
    }

    #[test]
    fn backtest_nonfinite_value_never_fires() {
        let spec = spec_with(DetectorKind::ZScore {
            sigma_threshold: 3.0,
            min_count: 5,
        });
        let mut points = flat_series(20, 100.0);
        points.push((t(20), f64::NAN));
        points.push((t(21), f64::INFINITY));
        let anomalies = backtest(&spec, &points).expect("valid spec");
        assert!(
            anomalies.is_empty(),
            "non-finite observations must not fire"
        );
    }
}
