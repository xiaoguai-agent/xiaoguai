/// Anomaly detector trait and standard implementations.
///
/// Each detector is stateful: it maintains its own rolling baseline and
/// cooldown tracking.  The caller feeds observations through `observe()`;
/// a return value of `Some(Anomaly)` means an alert should fire.
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::baseline::WelfordStats;

// ── Anomaly ────────────────────────────────────────────────────────────────

/// Description of a detected anomaly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Anomaly {
    /// Timestamp of the anomalous observation.
    pub ts: DateTime<Utc>,
    /// The observed value that triggered the alert.
    pub value: f64,
    /// The baseline mean at time of detection.
    pub baseline_mean: f64,
    /// The baseline std-dev at time of detection.
    pub baseline_std: f64,
    /// Signed deviation score (z-score or EWMA equivalent).
    pub score: f64,
    /// Human-readable description.
    pub description: String,
}

// ── Detector trait ─────────────────────────────────────────────────────────

/// Core detector interface.
pub trait Detector: Send + Sync {
    /// Feed one observation.  Returns `Some(Anomaly)` when a threshold is
    /// crossed and the cooldown period has elapsed since the last alert.
    fn observe(&mut self, ts: DateTime<Utc>, value: f64) -> Option<Anomaly>;
}

// ── Shared cooldown helper ──────────────────────────────────────────────────

/// Tracks whether enough time has elapsed since the last alert.
#[derive(Debug, Clone)]
struct Cooldown {
    period: Duration,
    last_fired: Option<DateTime<Utc>>,
}

impl Cooldown {
    fn new(period: Duration) -> Self {
        Self {
            period,
            last_fired: None,
        }
    }

    /// Returns `true` when an alert is allowed (not in cooldown).
    fn ready(&self, now: DateTime<Utc>) -> bool {
        match self.last_fired {
            None => true,
            Some(t) => now - t >= self.period,
        }
    }

    fn record(&mut self, now: DateTime<Utc>) {
        self.last_fired = Some(now);
    }
}


// ── ZScoreDetector ─────────────────────────────────────────────────────────

/// Fires when `|value − mean| / σ > sigma_threshold`.
///
/// Uses Welford's online algorithm for O(1) mean/variance updates.
/// Requires at least 2 observations before any alert can fire (σ is 0
/// with a single point).
#[derive(Debug, Clone)]
pub struct ZScoreDetector {
    /// Number of sigmas above which a point is anomalous.
    pub sigma_threshold: f64,
    /// Minimum population required before the detector arms.
    pub min_count: u64,
    /// Cooldown between successive alerts.
    pub cool_off: Duration,
    stats: WelfordStats,
    cooldown: Cooldown,
}

impl ZScoreDetector {
    /// Create with explicit parameters.
    #[must_use]
    pub fn new(sigma_threshold: f64, min_count: u64, cool_off: Duration) -> Self {
        Self {
            sigma_threshold,
            min_count,
            cool_off,
            stats: WelfordStats::new(),
            cooldown: Cooldown::new(cool_off),
        }
    }

    /// Convenience: 3-σ threshold, arm after 5 observations, 5-min cooldown.
    #[must_use]
    pub fn default_config() -> Self {
        Self::new(3.0, 5, Duration::minutes(5))
    }
}

impl Detector for ZScoreDetector {
    fn observe(&mut self, ts: DateTime<Utc>, value: f64) -> Option<Anomaly> {
        // Update baseline *before* scoring so the new value is part of the
        // distribution (Welford is consistent with this; scores are still
        // meaningful for large N).
        self.stats.update(value);

        let count = self.stats.count();
        if count < self.min_count {
            debug!(count, "ZScore not yet armed");
            return None;
        }

        let mean = self.stats.mean();
        let std = self.stats.std_dev();

        // Avoid division by zero when all values are identical.
        if std < f64::EPSILON {
            return None;
        }

        let score = (value - mean) / std;
        if score.abs() > self.sigma_threshold && self.cooldown.ready(ts) {
            self.cooldown.record(ts);
            let severity = if score.abs() > self.sigma_threshold * 2.0 {
                "critical"
            } else {
                "high"
            };
            if let Some(ctr) = xiaoguai_observability::anomaly_detections_total() {
                ctr.with_label_values(&["zscore", severity]).inc();
            }
            let anomaly = Anomaly {
                ts,
                value,
                baseline_mean: mean,
                baseline_std: std,
                score,
                description: format!(
                    "Z-score {score:.2} exceeds threshold {:.2} (mean={mean:.4}, σ={std:.4})",
                    self.sigma_threshold
                ),
            };
            debug!(?anomaly, "ZScoreDetector fired");
            return Some(anomaly);
        }
        None
    }
}

// ── EwmaDetector ───────────────────────────────────────────────────────────

/// Exponentially-weighted moving-average detector.
///
/// Tracks a smoothed estimate of the series mean and variance using the EWMA
/// update rule:
/// ```text
///   mean_t   = α · value + (1 − α) · mean_{t-1}
///   var_t    = (1 − α) · (var_{t-1} + α · (value − mean_{t-1})²)
///   score    = (value − mean_t) / sqrt(var_t)
/// ```
/// This adapts to slow trends while remaining sensitive to sudden spikes.
#[derive(Debug, Clone)]
pub struct EwmaDetector {
    /// Smoothing factor in (0, 1).  Higher = faster adaptation.
    pub alpha: f64,
    /// Number of sigmas above which a point is anomalous.
    pub sigma_threshold: f64,
    /// Cooldown between successive alerts.
    pub cool_off: Duration,
    ewma_mean: Option<f64>,
    ewma_var: Option<f64>,
    count: u64,
    /// Minimum observations before arming.
    pub min_count: u64,
    cooldown: Cooldown,
}

impl EwmaDetector {
    /// Create with explicit parameters.
    #[must_use]
    pub fn new(alpha: f64, sigma_threshold: f64, min_count: u64, cool_off: Duration) -> Self {
        assert!(
            (0.0..=1.0).contains(&alpha),
            "alpha must be in (0, 1), got {alpha}"
        );
        Self {
            alpha,
            sigma_threshold,
            cool_off,
            ewma_mean: None,
            ewma_var: None,
            count: 0,
            min_count,
            cooldown: Cooldown::new(cool_off),
        }
    }

    /// Convenience: α=0.1, 3-σ, arm after 5 obs, 5-min cooldown.
    #[must_use]
    pub fn default_config() -> Self {
        Self::new(0.1, 3.0, 5, Duration::minutes(5))
    }
}

impl Detector for EwmaDetector {
    fn observe(&mut self, ts: DateTime<Utc>, value: f64) -> Option<Anomaly> {
        self.count += 1;

        match (self.ewma_mean, self.ewma_var) {
            (None, _) => {
                // Seed with first observation; variance unknown yet.
                self.ewma_mean = Some(value);
                self.ewma_var = Some(0.0);
                return None;
            }
            (Some(prev_mean), Some(prev_var)) => {
                // Score against the PREVIOUS baseline (before the new value
                // contaminates the estimate) so a spike doesn't absorb its
                // own signal.
                let prev_std = prev_var.sqrt();

                // Update EWMA state.
                let new_mean = self.alpha * value + (1.0 - self.alpha) * prev_mean;
                let new_var =
                    (1.0 - self.alpha) * (prev_var + self.alpha * (value - prev_mean).powi(2));
                self.ewma_mean = Some(new_mean);
                self.ewma_var = Some(new_var);

                if self.count < self.min_count {
                    debug!(count = self.count, "EWMA not yet armed");
                    return None;
                }

                if prev_std < f64::EPSILON {
                    return None;
                }

                let score = (value - prev_mean) / prev_std;
                if score.abs() > self.sigma_threshold && self.cooldown.ready(ts) {
                    self.cooldown.record(ts);
                    let severity = if score.abs() > self.sigma_threshold * 2.0 {
                        "critical"
                    } else {
                        "high"
                    };
                    if let Some(ctr) = xiaoguai_observability::anomaly_detections_total() {
                        ctr.with_label_values(&["ewma", severity]).inc();
                    }
                    let anomaly = Anomaly {
                        ts,
                        value,
                        baseline_mean: prev_mean,
                        baseline_std: prev_std,
                        score,
                        description: format!(
                            "EWMA score {score:.2} exceeds threshold {:.2} (ewma_mean={prev_mean:.4}, ewma_σ={prev_std:.4})",
                            self.sigma_threshold
                        ),
                    };
                    debug!(?anomaly, "EwmaDetector fired");
                    return Some(anomaly);
                }
            }
            // Unreachable: both are set together.
            (Some(_), None) => unreachable!(),
        }
        None
    }
}

// ── Unit tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    // ── ZScoreDetector ────────────────────────────────────────────────────

    #[test]
    fn zscore_constant_series_no_anomaly() {
        let mut det = ZScoreDetector::new(3.0, 5, Duration::seconds(0));
        for i in 0..50 {
            let result = det.observe(t(i), 100.0);
            assert!(result.is_none(), "constant series should never fire");
        }
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn zscore_spike_on_flat_baseline_flagged() {
        let mut det = ZScoreDetector::new(3.0, 10, Duration::seconds(0));
        // Build a stable baseline around 100
        for i in 0..20 {
            det.observe(t(i), 100.0 + (i % 3) as f64);
        }
        // Spike to 200 — should fire
        let result = det.observe(t(20), 200.0);
        assert!(result.is_some(), "spike should be flagged");
        let a = result.unwrap();
        assert!(a.score > 3.0);
    }

    #[test]
    fn zscore_not_armed_before_min_count() {
        let mut det = ZScoreDetector::new(1.0, 10, Duration::seconds(0));
        for i in 0..9 {
            // Even with a huge value, should not fire
            let r = det.observe(t(i), if i == 5 { 9999.0 } else { 1.0 });
            assert!(r.is_none(), "should not fire before min_count");
        }
    }

    #[test]
    fn zscore_cooldown_suppresses_repeat_alerts() {
        let mut det = ZScoreDetector::new(3.0, 5, Duration::seconds(60));
        // Build a stable alternating baseline (9 / 11) so σ ≈ 1.0 and no
        // individual baseline point triggers an alert.  A spike of 100 000
        // scores ≈ 99 990σ and stays anomalous even after a few spike values
        // are folded into the running Welford stats.
        for i in 0..100i64 {
            det.observe(t(i), if i % 2 == 0 { 9.0 } else { 11.0 });
        }

        // First spike — should fire
        let r1 = det.observe(t(100), 100_000.0);
        assert!(r1.is_some(), "first spike should fire");
        // Second spike 1 s later — should be suppressed (cooldown = 60 s)
        let r2 = det.observe(t(101), 100_000.0);
        assert!(
            r2.is_none(),
            "second spike within cooldown should be suppressed"
        );
        // Third spike after cooldown expires — should fire again
        let r3 = det.observe(t(100 + 61), 100_000.0);
        assert!(r3.is_some(), "spike after cooldown should fire");
    }

    // ── EwmaDetector ──────────────────────────────────────────────────────

    #[test]
    fn ewma_constant_series_no_anomaly() {
        let mut det = EwmaDetector::new(0.2, 3.0, 5, Duration::seconds(0));
        for i in 0..100 {
            let r = det.observe(t(i), 50.0);
            assert!(r.is_none(), "constant series should never fire");
        }
    }

    #[test]
    fn ewma_spike_flagged() {
        // Feed alternating values so EWMA variance is non-zero, then spike.
        // Scoring uses previous (pre-spike) baseline, so even α=0.2 works.
        let mut det = EwmaDetector::new(0.2, 3.0, 10, Duration::seconds(0));
        for i in 0..30 {
            // Alternating 49 / 51 → EWMA var converges to ~4, σ≈2
            det.observe(t(i), if i % 2 == 0 { 49.0 } else { 51.0 });
        }
        // Spike to 5000 → prev σ ≈ 2, score ≈ (5000-50)/2 ≫ 3
        let r = det.observe(t(30), 5000.0);
        assert!(r.is_some(), "large spike should be flagged by EWMA");
    }

    #[test]
    fn ewma_cooldown_suppresses_repeat() {
        // Alternating baseline 9/11 so σ ≈ 1.  Score uses pre-value baseline,
        // so a spike of 50 000 clearly exceeds 3-σ.
        // After the first spike is suppressed, feed normal values to drain the
        // EWMA back towards the baseline before the re-fire check.
        let mut det = EwmaDetector::new(0.2, 3.0, 5, Duration::seconds(60));
        for i in 0..20i64 {
            det.observe(t(i), if i % 2 == 0 { 9.0 } else { 11.0 });
        }
        // First spike — should fire
        let r1 = det.observe(t(20), 50_000.0);
        assert!(r1.is_some(), "first spike should fire");
        // Second spike 1 s later — cooldown active
        let r2 = det.observe(t(21), 50_000.0);
        assert!(r2.is_none(), "cooldown should suppress");
        // Feed 60 normal values while waiting for cooldown to expire so the
        // EWMA recovers towards the baseline (t=22 … t=81).
        for i in 22i64..82 {
            det.observe(t(i), if i % 2 == 0 { 9.0 } else { 11.0 });
        }
        // Third spike after cooldown (60 s + 2 s margin = t=82) — should fire
        let r3 = det.observe(t(82), 50_000.0);
        assert!(r3.is_some(), "after cooldown should re-fire");
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn ewma_trending_series_flagged_when_deviation_large() {
        // Feed a rising series with small noise so EWMA variance is non-zero.
        // Then drop to zero — should exceed the threshold using pre-value baseline.
        let mut det = EwmaDetector::new(0.2, 2.5, 10, Duration::seconds(0));
        for i in 0i64..40 {
            // Rise 100, 101, 102, … with ±0.5 alternating noise
            let noise = if i % 2 == 0 { 0.5 } else { -0.5 };
            det.observe(t(i), 100.0 + i as f64 + noise);
        }
        // Sudden catastrophic drop to near-zero while baseline expects ~140
        let r = det.observe(t(40), 0.0);
        assert!(r.is_some(), "sudden drop on trending series should fire");
    }
}
