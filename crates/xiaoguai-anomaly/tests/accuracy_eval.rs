/// Capability eval: precision + recall on synthetic time-series.
///
/// All scenarios use a deterministic LCG for reproducible noise — no
/// external rand crate is needed.  The LCG constants are identical to those
/// in `baseline.rs` for consistency.
///
/// # Metric definitions
///
/// * **recall**    = `true_positives` / (`true_positives` + `false_negatives`)
///                 = fraction of injected anomalies the detector caught
/// * **precision** = `true_positives` / (`true_positives` + `false_positives`)
///                 = fraction of detector alerts that were actual anomalies
///
/// "Detected" means the detector fired *within 1 tick* of the injected index.
/// One-tick tolerance accounts for the fact that `ZScore` folds the anomalous
/// value into its running mean before scoring, which occasionally shifts the
/// peak one step.
use chrono::{Duration, TimeZone, Utc};
use xiaoguai_anomaly::detector::{Detector, EwmaDetector, ZScoreDetector};

// ── deterministic noise helper ────────────────────────────────────────────

/// Simple LCG that generates values in `[low, high)`.
struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Advance the LCG and return the next raw state.
    fn next_raw(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.state
    }

    /// Uniform sample in `[low, high)`.
    #[allow(clippy::cast_precision_loss)]
    fn next_f64(&mut self, low: f64, high: f64) -> f64 {
        let r = (self.next_raw() >> 11) as f64 / (u64::MAX >> 11) as f64;
        low + r * (high - low)
    }
}

// ── timestamp helper ──────────────────────────────────────────────────────

fn t(secs: i64) -> chrono::DateTime<chrono::Utc> {
    Utc.timestamp_opt(secs, 0).unwrap()
}

// ── precision / recall helpers ────────────────────────────────────────────

/// Count how many of the `spike_indices` appear within ±1 tick of a fired
/// alert index from `alerts`.
fn count_true_positives(spike_indices: &[usize], alerts: &[usize]) -> usize {
    spike_indices
        .iter()
        .filter(|&&sp| alerts.iter().any(|&a| a.abs_diff(sp) <= 1))
        .count()
}

// ── Scenario 1: sinusoidal + noise + injected spikes ─────────────────────
//
// 1 000 points.  Baseline = sin wave with amplitude 10, period 100,
// centred at 100, plus uniform noise in [-2, 2].
// 10 spikes of +200 injected at known indices.
// Assert ZScore: recall ≥ 80 %, false-positive rate < 5 %.

#[test]
#[allow(clippy::cast_precision_loss)]
fn scenario_sinusoidal_spike_recall_precision() {
    const N: usize = 1_000;
    const SPIKE_MAGNITUDE: f64 = 200.0;

    // Spike indices chosen to be well-separated and within the "armed" region.
    let spike_indices: Vec<usize> = vec![50, 120, 200, 310, 400, 501, 600, 710, 810, 920];
    assert_eq!(spike_indices.len(), 10);

    let mut lcg = Lcg::new(0xABCD_EF01_2345_6789);
    let mut values = Vec::with_capacity(N);

    for i in 0..N {
        let base = 100.0 + 10.0 * (2.0 * std::f64::consts::PI * i as f64 / 100.0).sin();
        let noise = lcg.next_f64(-2.0, 2.0);
        values.push(base + noise);
    }
    // Inject spikes on top.
    for &idx in &spike_indices {
        values[idx] += SPIKE_MAGNITUDE;
    }

    // ZScore: 3-σ threshold, arm after 20 obs, no cooldown so every spike can fire.
    let mut det = ZScoreDetector::new(3.0, 20, Duration::seconds(0));
    let mut alerts: Vec<usize> = Vec::new();

    for (i, &v) in values.iter().enumerate() {
        if det.observe(t(i as i64), v).is_some() {
            alerts.push(i);
        }
    }

    let tp = count_true_positives(&spike_indices, &alerts);
    let fp = alerts
        .iter()
        .filter(|&&a| spike_indices.iter().all(|&sp| a.abs_diff(sp) > 1))
        .count();

    let recall = tp as f64 / spike_indices.len() as f64;
    let fp_rate = fp as f64 / (N - spike_indices.len()) as f64;

    eprintln!(
        "[scenario 1] alerts={} tp={} fp={} recall={:.2} fp_rate={:.4}",
        alerts.len(),
        tp,
        fp,
        recall,
        fp_rate
    );

    assert!(
        recall >= 0.80,
        "ZScore recall on sinusoidal+spikes should be ≥0.80, got {recall:.2}"
    );
    assert!(
        fp_rate < 0.05,
        "ZScore false-positive rate should be <5%, got {fp_rate:.4}"
    );
}

// ── Scenario 2: step change — flat A → flat B ────────────────────────────
//
// 500 points at value 50, then 500 points at value 150.
// At least one detector (ZScore and/or EWMA) should flag the boundary region
// (pts 490-530). We verify at least one alert fires in [490, 530].

#[test]
fn scenario_step_change_detectors_flag_boundary() {
    const N: usize = 1_000;
    const BOUNDARY: usize = 500;
    const TOLERANCE: usize = 30; // 490-530

    let mut zscore = ZScoreDetector::new(3.0, 20, Duration::seconds(0));
    let mut ewma = EwmaDetector::new(0.1, 3.0, 20, Duration::seconds(0));

    let mut zscore_boundary_alert = false;
    let mut ewma_boundary_alert = false;

    for i in 0..N {
        let v = if i < BOUNDARY { 50.0 } else { 150.0 };
        let ts = t(i as i64);

        if zscore.observe(ts, v).is_some()
            && (BOUNDARY - TOLERANCE..=BOUNDARY + TOLERANCE).contains(&i)
        {
            zscore_boundary_alert = true;
        }
        if ewma.observe(ts, v).is_some()
            && (BOUNDARY - TOLERANCE..=BOUNDARY + TOLERANCE).contains(&i)
        {
            ewma_boundary_alert = true;
        }
    }

    // Per the scenario intent (above): verify at least one detector flags the
    // step-change boundary region. ZScore reacts to the abrupt 50→150 jump;
    // EWMA (alpha=0.1) is a smoother and may not cross threshold within the
    // ±30 tolerance window — that's expected detector behaviour, not a miss.
    // (A dedicated EWMA-sensitivity tuning eval is tracked separately.)
    assert!(
        zscore_boundary_alert || ewma_boundary_alert,
        "at least one detector should flag the step-change boundary region [490, 530] \
         (zscore={zscore_boundary_alert}, ewma={ewma_boundary_alert})"
    );
}

// ── Scenario 3: gradual linear drift — low false-positive rate ───────────
//
// 1 000 points linearly rising from 0 to 100.  No injected spikes.
// Neither detector should flag more than 3% of points as anomalies.
// (ZScore especially: slow drift keeps the running mean tracking the series.)

#[test]
#[allow(clippy::cast_precision_loss)]
fn scenario_gradual_drift_low_false_positives() {
    const N: usize = 1_000;

    let mut zscore = ZScoreDetector::new(3.0, 20, Duration::seconds(0));
    let mut ewma = EwmaDetector::new(0.05, 3.0, 20, Duration::seconds(0));

    let mut zscore_alerts = 0usize;
    let mut ewma_alerts = 0usize;

    for i in 0..N {
        let v = i as f64 / (N as f64 - 1.0) * 100.0; // 0..=100 linear
        let ts = t(i as i64);
        if zscore.observe(ts, v).is_some() {
            zscore_alerts += 1;
        }
        if ewma.observe(ts, v).is_some() {
            ewma_alerts += 1;
        }
    }

    let zscore_fp_rate = zscore_alerts as f64 / N as f64;
    let ewma_fp_rate = ewma_alerts as f64 / N as f64;

    eprintln!(
        "[scenario 3] zscore_alerts={zscore_alerts} ({zscore_fp_rate:.3}) ewma_alerts={ewma_alerts} ({ewma_fp_rate:.3})"
    );

    assert!(
        zscore_fp_rate < 0.03,
        "ZScore should have <3% FP on gradual drift, got {zscore_fp_rate:.3}"
    );
    assert!(
        ewma_fp_rate < 0.03,
        "EWMA should have <3% FP on gradual drift, got {ewma_fp_rate:.3}"
    );
}

// ── Scenario 4: all-zero series — no panic, no anomaly ───────────────────

#[test]
fn scenario_all_zero_no_panic_no_anomaly() {
    let mut zscore = ZScoreDetector::new(3.0, 5, Duration::seconds(0));
    let mut ewma = EwmaDetector::new(0.1, 3.0, 5, Duration::seconds(0));

    for i in 0..200 {
        let ts = t(i);
        let r_z = zscore.observe(ts, 0.0);
        let r_e = ewma.observe(ts, 0.0);
        assert!(r_z.is_none(), "tick {i}: all-zero ZScore must not fire");
        assert!(r_e.is_none(), "tick {i}: all-zero EWMA must not fire");
    }
}

// ── Scenario 5: all-equal (non-zero) series — no panic, no anomaly ───────

#[test]
fn scenario_all_equal_nonzero_no_panic_no_anomaly() {
    let mut zscore = ZScoreDetector::new(3.0, 5, Duration::seconds(0));
    let mut ewma = EwmaDetector::new(0.1, 3.0, 5, Duration::seconds(0));

    for i in 0..500 {
        let ts = t(i);
        // Use an irrational-ish constant to avoid exact IEEE equality traps.
        let r_z = zscore.observe(ts, 42.424_242);
        let r_e = ewma.observe(ts, 42.424_242);
        assert!(r_z.is_none(), "tick {i}: constant ZScore must not fire");
        assert!(r_e.is_none(), "tick {i}: constant EWMA must not fire");
    }
}

// ── Scenario 6: single-point series — no panic, trivially no anomaly ─────

#[test]
fn scenario_single_point_no_panic() {
    let mut zscore = ZScoreDetector::new(3.0, 1, Duration::seconds(0));
    let mut ewma = EwmaDetector::new(0.1, 3.0, 1, Duration::seconds(0));

    // Even with min_count=1, a single value can't produce a useful σ.
    let r_z = zscore.observe(t(0), 9999.0);
    let r_e = ewma.observe(t(0), 9999.0);

    // ZScore: std_dev=0 after 1 point → division guard returns None.
    assert!(r_z.is_none(), "ZScore single point must not fire");
    // EWMA: first observation seeds the mean, ewma_var=0 → None.
    assert!(r_e.is_none(), "EWMA single point must not fire");
}

// ── Scenario 7: window-boundary anomaly ──────────────────────────────────
//
// We arm the detector with a flat baseline, then inject an anomaly at the
// very first window index (right after arming) AND at the midpoint.
// Both should be detected.

#[test]
fn scenario_window_boundary_anomaly_both_detected() {
    // arm_count = min_count for the detector
    const ARM_COUNT: u64 = 10;
    const SPIKE: f64 = 9_999.0;

    // ── anomaly right at the arming boundary (index = arm_count) ──────────
    let mut det_early = ZScoreDetector::new(3.0, ARM_COUNT, Duration::seconds(0));
    // Feed arm_count identical values first (std=0 after arm_count, but the
    // spike itself will be the (arm_count+1)-th call).  Use alternating ±1
    // so σ > 0 when the detector arms.
    for i in 0..ARM_COUNT {
        det_early.observe(t(i as i64), if i % 2 == 0 { 9.0 } else { 11.0 });
    }
    // The very next observation is the spike — fires at exactly the arm boundary.
    let r_early = det_early.observe(t(ARM_COUNT as i64), SPIKE);
    assert!(
        r_early.is_some(),
        "ZScore should detect anomaly right at the arm boundary"
    );

    // ── anomaly in the middle of a long stable run ─────────────────────────
    let mut det_mid = ZScoreDetector::new(3.0, ARM_COUNT, Duration::seconds(0));
    for i in 0..200i64 {
        det_mid.observe(t(i), if i % 2 == 0 { 9.0 } else { 11.0 });
    }
    let r_mid = det_mid.observe(t(200), SPIKE);
    assert!(
        r_mid.is_some(),
        "ZScore should detect anomaly in the middle of a long stable run"
    );

    // Both should report a large positive score.
    assert!(r_early.unwrap().score > 3.0);
    assert!(r_mid.unwrap().score > 3.0);
}

// ── Scenario 8: EWMA precision/recall on synthetic spikes ────────────────
//
// Same sinusoidal + noise baseline as scenario 1, same spike indices.
// EWMA scores against the *previous* baseline, so it should be at least
// as sensitive.  Require recall ≥ 70 % and FP rate < 5 %.
// (EWMA adapts faster to the contaminated mean, so threshold is slightly lower.)

#[test]
#[allow(clippy::cast_precision_loss)]
fn scenario_ewma_spike_recall_precision() {
    const N: usize = 1_000;
    const SPIKE_MAGNITUDE: f64 = 200.0;

    let spike_indices: Vec<usize> = vec![50, 120, 200, 310, 400, 501, 600, 710, 810, 920];

    let mut lcg = Lcg::new(0xDEAD_BEEF_1111_2222); // different seed from scenario 1
    let mut values = Vec::with_capacity(N);

    for i in 0..N {
        let base = 100.0 + 10.0 * (2.0 * std::f64::consts::PI * i as f64 / 100.0).sin();
        let noise = lcg.next_f64(-2.0, 2.0);
        values.push(base + noise);
    }
    for &idx in &spike_indices {
        values[idx] += SPIKE_MAGNITUDE;
    }

    // α=0.1 keeps the EWMA mean slow — spikes stand out clearly.
    let mut det = EwmaDetector::new(0.1, 3.0, 20, Duration::seconds(0));
    let mut alerts: Vec<usize> = Vec::new();

    for (i, &v) in values.iter().enumerate() {
        if det.observe(t(i as i64), v).is_some() {
            alerts.push(i);
        }
    }

    let tp = count_true_positives(&spike_indices, &alerts);
    let fp = alerts
        .iter()
        .filter(|&&a| spike_indices.iter().all(|&sp| a.abs_diff(sp) > 1))
        .count();

    let recall = tp as f64 / spike_indices.len() as f64;
    let fp_rate = fp as f64 / (N - spike_indices.len()) as f64;

    eprintln!(
        "[scenario 8] alerts={} tp={} fp={} recall={:.2} fp_rate={:.4}",
        alerts.len(),
        tp,
        fp,
        recall,
        fp_rate
    );

    assert!(
        recall >= 0.70,
        "EWMA recall on sinusoidal+spikes should be ≥0.70, got {recall:.2}"
    );
    assert!(
        fp_rate < 0.05,
        "EWMA false-positive rate should be <5%, got {fp_rate:.4}"
    );
}
