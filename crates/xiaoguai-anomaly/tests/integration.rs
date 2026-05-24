/// Integration tests for xiaoguai-anomaly.
///
/// These tests sit outside the crate modules and exercise the full public API.
use chrono::{Duration, TimeZone, Utc};
use xiaoguai_anomaly::{
    baseline::WelfordStats,
    detector::{Detector, EwmaDetector, ZScoreDetector},
    registry::{AnomalyRegistry, InMemoryStore},
    series::TimeSeries,
    spec::{ActionRef, AnomalySpec, DetectorKind},
};

// ── helpers ───────────────────────────────────────────────────────────────

fn t(secs: i64) -> chrono::DateTime<chrono::Utc> {
    Utc.timestamp_opt(secs, 0).unwrap()
}

fn orders_spec(id: &str, detector: DetectorKind, cool_off: Duration) -> AnomalySpec {
    AnomalySpec {
        id: id.to_string(),
        kpi_query: "SELECT COUNT(*) FROM orders".to_string(),
        window: Duration::hours(2),
        detector,
        cool_off,
        on_anomaly: ActionRef::WakeSession {
            session: "ops-agent".to_string(),
            prompt_template: "Order anomaly: {anomaly}".to_string(),
        },
    }
}

// ── T1: Constant series → no anomaly ─────────────────────────────────────

#[test]
fn constant_series_produces_no_anomaly() {
    let mut det = ZScoreDetector::new(3.0, 5, Duration::seconds(0));
    for i in 0..200 {
        let r = det.observe(t(i), 100.0);
        assert!(r.is_none(), "tick {i}: constant series must not fire");
    }
}

// ── T2: Trending series under EWMA → flagged when trend exceeded ──────────

#[test]
#[allow(clippy::cast_precision_loss)]
fn ewma_trending_series_flags_sudden_break() {
    // Slowly rising series with alternating ±1 noise so EWMA variance is
    // non-zero.  The detector uses pre-value baseline for scoring so even a
    // slow α adapts without absorbing the spike.
    let mut det = EwmaDetector::new(0.2, 2.5, 10, Duration::seconds(0));
    for i in 0i64..50 {
        let noise = if i % 2 == 0 { 1.0 } else { -1.0 };
        det.observe(t(i), 200.0 + i as f64 * 2.0 + noise);
    }
    // Value drops to 0 — catastrophic break in trend (prev mean ≈ 298).
    let r = det.observe(t(50), 0.0);
    assert!(
        r.is_some(),
        "sudden drop on trending series must be flagged by EWMA"
    );
    let a = r.unwrap();
    assert!(
        a.score < -2.5,
        "score should be large negative: {}",
        a.score
    );
}

// ── T3: Spike on flat baseline → flagged ──────────────────────────────────

#[test]
#[allow(clippy::cast_precision_loss)]
fn zscore_spike_on_flat_baseline() {
    let mut det = ZScoreDetector::new(3.0, 10, Duration::seconds(0));
    // Stable baseline around 50 ± 1
    for i in 0i64..30 {
        det.observe(t(i), 50.0 + (i % 3 - 1) as f64);
    }
    // Spike to 1000
    let r = det.observe(t(30), 1000.0);
    assert!(r.is_some(), "spike must be flagged");
    let a = r.unwrap();
    assert!(a.value > 900.0);
    assert!(a.score > 3.0);
}

// ── T4: Welford incremental matches batch for 1000 random points ──────────

#[test]
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn welford_matches_batch_1000_points() {
    // Reproducible deterministic LCG (no rand crate needed).
    let mut state: u64 = 0x1234_5678_9ABC_DEF0;
    let n = 1000usize;
    let mut welford = WelfordStats::new();
    let mut values = Vec::with_capacity(n);

    for _ in 0..n {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let v = (state >> 11) as f64 / (u64::MAX >> 11) as f64 * 500.0 - 250.0; // [-250, 250)
        values.push(v);
        welford.update(v);
    }

    let batch_mean: f64 = values.iter().sum::<f64>() / n as f64;
    let batch_var: f64 = values.iter().map(|x| (x - batch_mean).powi(2)).sum::<f64>() / n as f64;

    assert!(
        (welford.mean() - batch_mean).abs() < 1e-9,
        "mean drift: welford={} batch={batch_mean}",
        welford.mean()
    );
    assert!(
        (welford.variance() - batch_var).abs() < 1e-9,
        "variance drift: welford={} batch={batch_var}",
        welford.variance()
    );
}

// ── T5: Anomaly cooldown ──────────────────────────────────────────────────

#[test]
fn anomaly_cooldown_suppresses_re_fire_within_window() {
    let mut reg = AnomalyRegistry::new(Box::new(InMemoryStore::default()));
    reg.register(orders_spec(
        "cooldown_test",
        DetectorKind::ZScore {
            sigma_threshold: 3.0,
            min_count: 5,
        },
        Duration::seconds(60), // 1-minute cooldown
    ));

    // Build a stable alternating baseline (9/11) so σ ≈ 1.  Each individual
    // baseline observation is only ±1σ, so no premature firing.
    for i in 0i64..100 {
        reg.observe("cooldown_test", t(i), if i % 2 == 0 { 9.0 } else { 11.0 });
    }

    // First spike — should fire
    let r1 = reg.observe("cooldown_test", t(100), 1_000_000.0);
    assert!(r1.is_some(), "first spike must fire");

    // Immediate second spike — cooldown active, must be suppressed
    let r2 = reg.observe("cooldown_test", t(101), 1_000_000.0);
    assert!(
        r2.is_none(),
        "second spike within cooldown must be suppressed"
    );

    // Feed normal values to drain the spike contamination, then wait out the
    // cooldown (t=102..t=161 = 60 recovery ticks, total elapsed ≥ 60 s).
    for i in 102i64..162 {
        reg.observe("cooldown_test", t(i), if i % 2 == 0 { 9.0 } else { 11.0 });
    }

    // Third spike 1 second after cooldown expires — must fire again
    let r3 = reg.observe("cooldown_test", t(162), 1_000_000.0);
    assert!(r3.is_some(), "spike after cooldown must re-fire");

    // Exactly two anomalies recorded
    assert_eq!(reg.recorded_anomalies().len(), 2);
}

// ── T6: TimeSeries prune semantics ───────────────────────────────────────

#[test]
fn time_series_prunes_on_push() {
    let mut ts = TimeSeries::new(Duration::minutes(5));
    // window = 300 s
    ts.push(t(0), 1.0);
    ts.push(t(60), 2.0);
    ts.push(t(350), 3.0);
    // Push at t=600: cutoff = 300. t=0 < 300 → evicted. t=60 < 300 → evicted.
    // t=350 >= 300 → retained. t=600 retained.
    ts.push(t(600), 4.0);
    assert_eq!(ts.len(), 2, "t=350 and t=600 should remain");
    assert!((ts.points.front().unwrap().1 - 3.0).abs() < f64::EPSILON);
}

// ── T7: Registry with EWMA spec ──────────────────────────────────────────

#[test]
fn registry_ewma_spec_registered_and_fires() {
    let mut reg = AnomalyRegistry::new(Box::new(InMemoryStore::default()));
    reg.register(orders_spec(
        "ewma_reg",
        DetectorKind::Ewma {
            alpha: 0.2,
            sigma_threshold: 2.5,
            min_count: 10,
        },
        Duration::seconds(0),
    ));

    for i in 0i64..30 {
        reg.observe("ewma_reg", t(i), 50.0 + (i % 2) as f64 * 0.2);
    }
    let r = reg.observe("ewma_reg", t(30), 5000.0);
    assert!(r.is_some(), "EWMA spike must fire through registry");
    let (spec, anomaly) = r.unwrap();
    assert_eq!(spec.id, "ewma_reg");
    assert!(anomaly.score > 2.5);
}

// ── T8: Multiple specs coexist independently ─────────────────────────────

#[test]
fn multiple_specs_are_independent() {
    let mut reg = AnomalyRegistry::new(Box::new(InMemoryStore::default()));
    reg.register(orders_spec(
        "spec_a",
        DetectorKind::ZScore {
            sigma_threshold: 3.0,
            min_count: 5,
        },
        Duration::seconds(0),
    ));
    reg.register(orders_spec(
        "spec_b",
        DetectorKind::ZScore {
            sigma_threshold: 3.0,
            min_count: 5,
        },
        Duration::seconds(0),
    ));

    // Build a baseline with natural variation (±5 around 100) so a
    // value of 103 is within 1σ and doesn't trigger spec_a.
    for i in 0i64..50 {
        let noise = ((i % 11) - 5) as f64; // -5..+5
        reg.observe("spec_a", t(i), 100.0 + noise);
        reg.observe("spec_b", t(i), 100.0 + noise);
    }
    // spec_a: 103 is well within ±3σ of a ±5 baseline
    let spec_a_fired = reg.observe("spec_a", t(50), 103.0).is_some();
    // spec_b: 999999 is clearly anomalous
    let spec_b_fired = reg.observe("spec_b", t(50), 999_999.0).is_some();

    assert!(
        !spec_a_fired,
        "spec_a should not fire on a value within ±3σ"
    );
    assert!(spec_b_fired, "spec_b should fire on extreme spike");
}
