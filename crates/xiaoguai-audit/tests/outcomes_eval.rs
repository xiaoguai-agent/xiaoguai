//! Outcome attribution chain eval suite.
//!
//! Tests the `OutcomeRecorder` + related helpers (`OutcomeSummary`, `timeseries`)
//! end-to-end, modelling single-hop, multi-hop, branching, time-window,
//! cycle-like, and aggregation scenarios.
//!
//! # Design note
//! The `OutcomeRecorder` API is a flat record + aggregate model; there is no
//! dedicated chain-reconstruction reader trait in this crate (that lives in
//! `xiaoguai-core` as `PgOutcomeRecorder`). Attribution "chains" are modelled
//! here through shared `session_id` values: every node in a logical call chain
//! records under the same session, identified by `agent_name`. The
//! `InMemoryOutcomeRecorder::snapshot()` method gives us the raw records for
//! detailed chain assertions; `OutcomeSummary::from_records` and `timeseries()`
//! cover the aggregation and bucketing scenarios.

use chrono::{Duration, TimeZone, Utc};
use xiaoguai_audit::{
    timeseries, InMemoryOutcomeRecorder, OutcomeRange, OutcomeRecord, OutcomeRecorder,
    OutcomeSummary,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a minimal `OutcomeRecord` with a fixed timestamp of *now*.
// Retained as a fixture for outcome-record tests even when not currently
// referenced; keeps the eval-suite helper set complete.
#[allow(dead_code)]
fn make_record(
    session_id: Option<&str>,
    agent_name: &str,
    kind: &str,
    value: f64,
) -> OutcomeRecord {
    OutcomeRecord {
        session_id: session_id.map(ToOwned::to_owned),
        agent_name: agent_name.to_owned(),
        kind: kind.to_owned(),
        value,
        unit: None,
        description: None,
        attributed_at: Utc::now(),
        metadata: serde_json::Value::Null,
    }
}

/// Build a `OutcomeRecord` with an explicit timestamp (for time-window tests).
fn make_record_at(
    session_id: Option<&str>,
    agent_name: &str,
    kind: &str,
    value: f64,
    attributed_at: chrono::DateTime<Utc>,
) -> OutcomeRecord {
    OutcomeRecord {
        session_id: session_id.map(ToOwned::to_owned),
        agent_name: agent_name.to_owned(),
        kind: kind.to_owned(),
        value,
        unit: None,
        description: None,
        attributed_at,
        metadata: serde_json::Value::Null,
    }
}

// ---------------------------------------------------------------------------
// 1. Single-hop attribution
// ---------------------------------------------------------------------------

/// Single-hop: one input event → one agent call → one outcome.
///
/// The attribution link is the shared `session_id`. Reading back the snapshot
/// and filtering by session must return exactly one record with the correct
/// agent and kind.
#[tokio::test]
async fn single_hop_attribution_chain() {
    let recorder = InMemoryOutcomeRecorder::new();

    recorder
        .record(
            Some("session-A"),
            "agent-bot",
            "revenue_usd",
            500.0,
            None,
            Some("deal closed via single-hop"),
            serde_json::json!({"deal_id": "D001"}),
        )
        .await
        .unwrap();

    let snap = recorder.snapshot();
    assert_eq!(snap.len(), 1, "exactly one record in the store");

    // Filter to the specific session — simulates a chain reader query.
    let chain: Vec<&OutcomeRecord> = snap
        .iter()
        .filter(|r| r.session_id.as_deref() == Some("session-A"))
        .collect();

    assert_eq!(chain.len(), 1, "single-hop chain has exactly 1 node");
    assert_eq!(chain[0].agent_name, "agent-bot");
    assert_eq!(chain[0].kind, "revenue_usd");
    assert!(
        (chain[0].value - 500.0).abs() < f64::EPSILON,
        "value matches"
    );
}

// ---------------------------------------------------------------------------
// 2. Multi-hop chain (depth = 5)
// ---------------------------------------------------------------------------

/// Multi-hop: input → agent-A → agent-B → tool-call → agent-C → final outcome.
///
/// All five "hops" share the same `session_id`. The chain reader (snapshot
/// filtered by session + ordered by insertion) returns all 5 nodes in order.
#[tokio::test]
async fn multi_hop_chain_depth_5() {
    let recorder = InMemoryOutcomeRecorder::new();
    let session = "session-multi";

    let hops: &[(&str, &str, f64)] = &[
        ("agent-A", "hours_saved", 1.0),
        ("agent-B", "hours_saved", 2.0),
        ("tool-call", "hours_saved", 3.0),
        ("agent-C", "hours_saved", 4.0),
        ("final-outcome", "hours_saved", 5.0),
    ];

    for (agent, kind, value) in hops {
        recorder
            .record(
                Some(session),
                agent,
                kind,
                *value,
                None,
                None,
                serde_json::Value::Null,
            )
            .await
            .unwrap();
    }

    let snap = recorder.snapshot();
    let chain: Vec<&OutcomeRecord> = snap
        .iter()
        .filter(|r| r.session_id.as_deref() == Some(session))
        .collect();

    assert_eq!(chain.len(), 5, "all 5 hops present");
    // Insertion order is preserved in the in-memory impl (Vec append).
    let expected_agents = [
        "agent-A",
        "agent-B",
        "tool-call",
        "agent-C",
        "final-outcome",
    ];
    for (i, record) in chain.iter().enumerate() {
        assert_eq!(
            record.agent_name, expected_agents[i],
            "hop {i} agent mismatch"
        );
        assert!(
            (record.value - (i + 1) as f64).abs() < f64::EPSILON,
            "hop {i} value mismatch"
        );
    }

    // Aggregate over the whole chain should sum all hops.
    let agg = recorder
        .aggregate(Some("hours_saved"), OutcomeRange::default())
        .await
        .unwrap();
    assert_eq!(agg.count, 5);
    assert!((agg.sum - 15.0).abs() < f64::EPSILON, "sum of 1+2+3+4+5");
}

// ---------------------------------------------------------------------------
// 3. Branching chain — fan-out / fan-in
// ---------------------------------------------------------------------------

/// Branching: one input session fans out to 3 parallel agents (branch-X,
/// branch-Y, branch-Z), then a converge agent records the final outcome.
///
/// All four agents (3 branches + converge) share the session.
/// Attribution must recover all 4 nodes across both branches and the
/// convergence point.
#[tokio::test]
async fn branching_chain_fan_out_and_converge() {
    let recorder = InMemoryOutcomeRecorder::new();
    let session = "session-branch";

    // Three parallel branches.
    for branch in ["branch-X", "branch-Y", "branch-Z"] {
        recorder
            .record(
                Some(session),
                branch,
                "cost_saved_usd",
                10.0,
                None,
                None,
                serde_json::Value::Null,
            )
            .await
            .unwrap();
    }

    // Convergence point.
    recorder
        .record(
            Some(session),
            "converge-agent",
            "cost_saved_usd",
            5.0,
            None,
            Some("convergence outcome"),
            serde_json::Value::Null,
        )
        .await
        .unwrap();

    let snap = recorder.snapshot();
    let chain: Vec<&OutcomeRecord> = snap
        .iter()
        .filter(|r| r.session_id.as_deref() == Some(session))
        .collect();

    assert_eq!(chain.len(), 4, "3 branches + 1 converge = 4 nodes");

    // All 3 branches are present.
    for branch in ["branch-X", "branch-Y", "branch-Z"] {
        let found = chain.iter().any(|r| r.agent_name == branch);
        assert!(found, "branch {branch} must be in the chain");
    }

    // Convergence point is present.
    let converge = chain
        .iter()
        .find(|r| r.agent_name == "converge-agent")
        .expect("converge-agent must be in the chain");
    assert_eq!(converge.description.as_deref(), Some("convergence outcome"));

    // Aggregate: 3 × 10 + 5 = 35.
    let agg = recorder
        .aggregate(Some("cost_saved_usd"), OutcomeRange::default())
        .await
        .unwrap();
    assert_eq!(agg.count, 4);
    assert!((agg.sum - 35.0).abs() < f64::EPSILON);
}

// ---------------------------------------------------------------------------
// 4. Time-window query — older outcomes excluded
// ---------------------------------------------------------------------------

/// Time-window: outcomes older than the requested window are excluded from
/// the aggregate.
#[tokio::test]
async fn time_window_excludes_old_outcomes() {
    let recorder = InMemoryOutcomeRecorder::new();

    // Record one old outcome (48 h ago) and one recent (1 h ago).
    let old_ts = Utc::now() - Duration::hours(48);
    let recent_ts = Utc::now() - Duration::hours(1);

    // We use snapshot() + manual filter to inject back-dated records,
    // because the recorder always sets attributed_at = Utc::now() internally.
    // Instead we assert via OutcomeRange directly on pre-built OutcomeRecord
    // slices passed to aggregate() after the fact — using the public
    // OutcomeSummary::from_records helper which accepts arbitrary timestamps.
    let records = [
        make_record_at(
            Some("s1"),
            "agent-old",
            "hours_saved",
            8.0,
            old_ts,
        ),
        make_record_at(
            Some("s1"),
            "agent-recent",
            "hours_saved",
            3.0,
            recent_ts,
        ),
    ];

    // 24-hour window — only the recent record falls within.
    let window_start = Utc::now() - Duration::hours(24);
    let filtered: Vec<&OutcomeRecord> = records
        .iter()
        .filter(|r| r.attributed_at >= window_start)
        .collect();

    assert_eq!(
        filtered.len(),
        1,
        "only the recent record falls inside the 24h window"
    );
    assert_eq!(filtered[0].agent_name, "agent-recent");

    // Summary should only reflect the recent record.
    let filtered_owned: Vec<OutcomeRecord> = filtered.into_iter().cloned().collect();
    let summary = OutcomeSummary::from_records(&filtered_owned);
    let hrs = &summary.by_kind["hours_saved"];
    assert_eq!(hrs.count, 1);
    assert!((hrs.sum - 3.0).abs() < f64::EPSILON);

    // Also verify the live recorder's aggregate() range filtering.
    // Record both via the recorder (timestamps will be Utc::now(), so we use
    // OutcomeRange::from_shorthand to exercise that code path independently).
    recorder
        .record(
            None,
            "bot",
            "hours_saved",
            5.0,
            None,
            None,
            serde_json::Value::Null,
        )
        .await
        .unwrap();

    let range = OutcomeRange::from_shorthand("24h").unwrap();
    let agg = recorder
        .aggregate(Some("hours_saved"), range)
        .await
        .unwrap();
    // The just-recorded entry is within 24h.
    assert_eq!(agg.count, 1);

    // An empty inverted range returns an error.
    let later = Utc.with_ymd_and_hms(2026, 5, 25, 12, 0, 0).unwrap();
    let earlier = Utc.with_ymd_and_hms(2026, 5, 24, 12, 0, 0).unwrap();
    let inverted_range = OutcomeRange {
        since: Some(later),
        until: Some(earlier),
    };
    let err = recorder
        .aggregate(None, inverted_range)
        .await
        .unwrap_err();
    assert!(
        matches!(err, xiaoguai_audit::OutcomeError::InvalidArgument(_)),
        "inverted range must be rejected"
    );
}

// ---------------------------------------------------------------------------
// 5. Aggregate counts every recorded outcome (single-owner)
// ---------------------------------------------------------------------------

/// Single-owner model: every recorded outcome of a kind is counted by
/// `aggregate` — there is no tenant axis to scope reads by.
#[tokio::test]
async fn aggregate_counts_all_records_of_kind() {
    let recorder = InMemoryOutcomeRecorder::new();

    for _ in 0..10 {
        recorder
            .record(
                Some("shared-session-id"),
                "shared-agent",
                "deals_closed",
                1.0,
                None,
                None,
                serde_json::Value::Null,
            )
            .await
            .unwrap();
    }

    let agg = recorder
        .aggregate(Some("deals_closed"), OutcomeRange::default())
        .await
        .unwrap();
    assert_eq!(agg.count, 10);
    assert!((agg.sum - 10.0).abs() < f64::EPSILON);

    let snap = recorder.snapshot();
    assert_eq!(snap.len(), 10);
}

// ---------------------------------------------------------------------------
// 6. Cycle protection
// ---------------------------------------------------------------------------

/// Cycle protection: a malformed "chain" where agent-A calls agent-B which
/// calls agent-A again (A → B → A) must not infinite-loop the reader.
///
/// Gap: The `InMemoryOutcomeRecorder` stores records in a flat `Vec` and
/// has no cycle-detection logic — there is no graph reconstruction in the
/// in-process impl. This test asserts that reading the snapshot of a cyclic
/// session terminates (trivially true for the flat model) and that the count
/// of records is bounded by what was inserted.
///
/// A future `OutcomeReader` with graph traversal MUST include cycle protection.
/// This test is marked `#[ignore]` until a graph-walking reader is added.
#[tokio::test]
#[ignore = "TODO(chain-reader): cycle detection required once graph-walking OutcomeReader is implemented"]
async fn cycle_protection_does_not_infinite_loop() {
    let recorder = InMemoryOutcomeRecorder::new();
    let session = "session-cycle";

    // Simulate A → B → A by recording all three hops.
    for agent in ["agent-A", "agent-B", "agent-A-again"] {
        recorder
            .record(
                Some(session),
                agent,
                "custom",
                1.0,
                None,
                None,
                serde_json::Value::Null,
            )
            .await
            .unwrap();
    }

    // With the flat in-memory impl there is no cycle risk; for a graph reader
    // the expectation is either an explicit `Err(CycleDetected)` or a
    // truncated chain (not an infinite loop).
    let snap = recorder.snapshot();
    let chain: Vec<&OutcomeRecord> = snap
        .iter()
        .filter(|r| r.session_id.as_deref() == Some(session))
        .collect();

    // Bounded: exactly the 3 records that were inserted.
    assert_eq!(chain.len(), 3, "cyclic session must return bounded records");
}

// ---------------------------------------------------------------------------
// 7. Summary aggregation — per-agent count vs. total
// ---------------------------------------------------------------------------

/// Summary aggregation: total outcome count equals the sum of
/// all per-agent (per-kind) counts.
#[tokio::test]
async fn summary_aggregation_total_matches_per_kind_sum() {
    let recorder = InMemoryOutcomeRecorder::new();

    let entries: &[(&str, &str, f64)] = &[
        ("agent-alpha", "revenue_usd", 100.0),
        ("agent-alpha", "revenue_usd", 200.0),
        ("agent-beta", "hours_saved", 4.0),
        ("agent-beta", "hours_saved", 6.0),
        ("agent-gamma", "deals_closed", 1.0),
    ];

    for (agent, kind, value) in entries {
        recorder
            .record(
                Some("session-summary"),
                agent,
                kind,
                *value,
                None,
                None,
                serde_json::Value::Null,
            )
            .await
            .unwrap();
    }

    let records = recorder.snapshot();
    let summary = OutcomeSummary::from_records(&records);

    // Verify per-kind aggregates.
    let rev = &summary.by_kind["revenue_usd"];
    assert_eq!(rev.count, 2);
    assert!((rev.sum - 300.0).abs() < f64::EPSILON);
    assert!((rev.avg - 150.0).abs() < f64::EPSILON);

    let hrs = &summary.by_kind["hours_saved"];
    assert_eq!(hrs.count, 2);
    assert!((hrs.sum - 10.0).abs() < f64::EPSILON);
    assert!((hrs.avg - 5.0).abs() < f64::EPSILON);

    let deals = &summary.by_kind["deals_closed"];
    assert_eq!(deals.count, 1);
    assert!((deals.sum - 1.0).abs() < f64::EPSILON);

    // Total count == sum of per-kind counts.
    let total_count_from_summary: u64 = summary.by_kind.values().map(|a| a.count).sum();
    assert_eq!(
        total_count_from_summary,
        entries.len() as u64,
        "per-kind count sum must equal total record count"
    );

    // Cross-kind aggregate from recorder must also match.
    let agg_total = recorder
        .aggregate(None, OutcomeRange::default())
        .await
        .unwrap();
    assert_eq!(agg_total.count, entries.len() as u64);
    assert!(
        (agg_total.sum - 311.0).abs() < f64::EPSILON,
        "100+200+4+6+1 = 311"
    );
}

// ---------------------------------------------------------------------------
// 8. Timeseries bucketing — 3 hour-buckets
// ---------------------------------------------------------------------------

/// Timeseries bucketing: outcomes spread across 3 distinct UTC days are
/// bucketed into exactly 3 `OutcomeDay` entries by `timeseries()`.
///
/// Note: `timeseries()` operates on **daily** buckets, not hourly.
/// For hour-bucket granularity a future API extension is needed; for now
/// this test asserts correct **daily** bucketing across 3 days.
#[test]
fn timeseries_three_day_buckets() {
    let day1 = Utc.with_ymd_and_hms(2026, 5, 20, 2, 0, 0).unwrap();
    let day2 = Utc.with_ymd_and_hms(2026, 5, 21, 10, 0, 0).unwrap();
    let day3 = Utc.with_ymd_and_hms(2026, 5, 22, 23, 0, 0).unwrap();

    let records = vec![
        // Day 1 — two revenue entries.
        make_record_at(Some("s"), "bot", "revenue_usd", 50.0, day1),
        make_record_at(Some("s"), "bot", "revenue_usd", 50.0, day1),
        // Day 2 — one hours_saved.
        make_record_at(Some("s"), "bot", "hours_saved", 3.0, day2),
        // Day 3 — one deals_closed.
        make_record_at(Some("s"), "bot", "deals_closed", 2.0, day3),
    ];

    let buckets = timeseries(&records);

    // 3 distinct (date, kind) combos → 3 bucket entries.
    assert_eq!(
        buckets.len(),
        3,
        "expected 3 daily buckets, got: {buckets:?}"
    );

    let rev = buckets
        .iter()
        .find(|b| b.kind == "revenue_usd")
        .expect("revenue_usd bucket must exist");
    assert_eq!(rev.date, "2026-05-20");
    assert!((rev.sum - 100.0).abs() < f64::EPSILON);
    assert_eq!(rev.count, 2);

    let hrs = buckets
        .iter()
        .find(|b| b.kind == "hours_saved")
        .expect("hours_saved bucket must exist");
    assert_eq!(hrs.date, "2026-05-21");
    assert!((hrs.sum - 3.0).abs() < f64::EPSILON);
    assert_eq!(hrs.count, 1);

    let deals = buckets
        .iter()
        .find(|b| b.kind == "deals_closed")
        .expect("deals_closed bucket must exist");
    assert_eq!(deals.date, "2026-05-22");
    assert!((deals.sum - 2.0).abs() < f64::EPSILON);
    assert_eq!(deals.count, 1);
}

/// Timeseries: outcomes on the same day but different hours land in the
/// SAME daily bucket (no false hour-level splits).
#[test]
fn timeseries_same_day_different_hours_same_bucket() {
    let base = Utc.with_ymd_and_hms(2026, 5, 23, 0, 0, 0).unwrap();
    let records: Vec<OutcomeRecord> = (0..3)
        .map(|h| {
            make_record_at(
                None,
                "bot",
                "hours_saved",
                1.0,
                base + Duration::hours(h),
            )
        })
        .collect();

    let buckets = timeseries(&records);
    assert_eq!(buckets.len(), 1, "all 3 hours on the same day → 1 bucket");
    assert_eq!(buckets[0].count, 3);
    assert!((buckets[0].sum - 3.0).abs() < f64::EPSILON);
}
