//! Integration tests for `xiaoguai-watch`.
//!
//! Tests use [`InMemorySource`] so no live database or HTTP server is needed.
//!
//! ## Scenarios
//!
//! 1. Same row returned twice within TTL → fires **once**.
//! 2. Row content changes → fires **again** (different fingerprint).
//! 3. Same row returned after TTL expiry → fires **again** (evicted).
//! 4. Multiple specs with the same row content → each spec fires independently
//!    (fingerprints are namespaced by spec id).
//! 5. Empty source produces no events.

use std::time::Duration;

use serde_json::json;
use xiaoguai_watch::{
    ActionRef, DedupCache, InMemorySource, WatchRunner, WatchSchedule, WatchSourceSpec, WatchSpec,
};

fn make_spec(id: &str, interval_ms: u64) -> WatchSpec {
    WatchSpec {
        id: id.to_string(),
        source: WatchSourceSpec::Sql {
            // content irrelevant — InMemorySource is used instead of SqlSource
            query: "SELECT 1".into(),
        },
        schedule: WatchSchedule::IntervalSecs {
            secs: interval_ms.max(1), // minimum 1 s for WatchRunner
        },
        on_match: ActionRef {
            action: "notify".into(),
            target: Some("test-channel".into()),
            params: serde_json::Map::new(),
        },
    }
}

// ---------------------------------------------------------------------------
// 1. Same row within TTL fires exactly once
// ---------------------------------------------------------------------------

#[tokio::test]
async fn same_row_within_ttl_fires_once() {
    let dedup = DedupCache::new(100, Duration::from_secs(3600)); // long TTL
    let mut runner = WatchRunner::with_dedup(dedup).channel_capacity(32);

    let spec = make_spec("dedup-once", 1);
    let source = InMemorySource::new(vec![serde_json::from_value(
        json!({"tenant": "acme", "dso": 72}),
    )
    .unwrap()]);
    runner.register(spec, source);
    let mut rx = runner.run();

    // First event should arrive quickly (tick fires on start).
    let first = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("first event timed out")
        .expect("channel closed");

    assert_eq!(first.spec_id, "dedup-once");
    assert_eq!(first.payload["dso"], 72);

    // Wait two more ticks — still the same row, still deduplicated.
    let second = tokio::time::timeout(Duration::from_millis(2500), rx.recv()).await;
    assert!(
        second.is_err(),
        "second event must be suppressed (duplicate within TTL)"
    );
}

// ---------------------------------------------------------------------------
// 2. Row content changes → fires again
// ---------------------------------------------------------------------------

#[tokio::test]
async fn changed_row_fires_again() {
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use xiaoguai_watch::{Match, SourceError, WatchSource};

    // Source that alternates between two different rows.
    struct AltSource {
        rows: [serde_json::Value; 2],
        idx: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl WatchSource for AltSource {
        async fn poll(&self) -> Result<Vec<Match>, SourceError> {
            let i = self.idx.fetch_add(1, Ordering::Relaxed) % 2;
            Ok(vec![Match::from_value(self.rows[i].clone())])
        }
    }

    let dedup = DedupCache::new(100, Duration::from_secs(3600));
    let mut runner = WatchRunner::with_dedup(dedup).channel_capacity(32);

    let spec = make_spec("dedup-changed", 1);
    let source = AltSource {
        rows: [json!({"dso": 61}), json!({"dso": 62})],
        idx: Arc::new(AtomicUsize::new(0)),
    };
    runner.register(spec, source);
    let mut rx = runner.run();

    let e1 = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("event 1 timed out")
        .unwrap();
    let e2 = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("event 2 timed out")
        .unwrap();

    assert_ne!(
        e1.payload, e2.payload,
        "different row content must produce distinct events"
    );
}

// ---------------------------------------------------------------------------
// 3. Row fires again after TTL expiry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn row_fires_again_after_ttl_expiry() {
    // Very short TTL — 150 ms.
    let dedup = DedupCache::new(100, Duration::from_millis(150));
    let mut runner = WatchRunner::with_dedup(dedup).channel_capacity(32);

    let spec = make_spec("dedup-ttl", 1);
    let source = InMemorySource::new(vec![serde_json::from_value(
        json!({"customer": "beta", "dso": 90}),
    )
    .unwrap()]);
    runner.register(spec, source);
    let mut rx = runner.run();

    // First event.
    let e1 = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("first event timed out")
        .unwrap();
    assert_eq!(e1.payload["customer"], "beta");

    // Wait for TTL to expire (300 ms > 150 ms TTL).
    tokio::time::sleep(Duration::from_millis(300)).await;

    // After expiry the same row should fire again.
    let e2 = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("second event (post-TTL) timed out")
        .unwrap();
    assert_eq!(e2.payload["customer"], "beta");
}

// ---------------------------------------------------------------------------
// 4. Two specs with the same row content fire independently
// ---------------------------------------------------------------------------

#[tokio::test]
async fn two_specs_same_row_are_independent() {
    let dedup = DedupCache::new(100, Duration::from_secs(3600));
    let mut runner = WatchRunner::with_dedup(dedup).channel_capacity(32);

    let row: serde_json::Map<String, serde_json::Value> =
        serde_json::from_value(json!({"metric": "latency", "value": 500})).unwrap();

    runner.register(
        make_spec("spec-a", 1),
        InMemorySource::new(vec![row.clone()]),
    );
    runner.register(make_spec("spec-b", 1), InMemorySource::new(vec![row]));
    let mut rx = runner.run();

    let mut seen_ids = std::collections::HashSet::new();
    for _ in 0..2 {
        let ev = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("event timed out")
            .unwrap();
        seen_ids.insert(ev.spec_id.clone());
    }
    assert!(seen_ids.contains("spec-a"), "spec-a must fire");
    assert!(
        seen_ids.contains("spec-b"),
        "spec-b must fire independently"
    );
}

// ---------------------------------------------------------------------------
// 5. Empty source produces no events
// ---------------------------------------------------------------------------

#[tokio::test]
async fn empty_source_no_events() {
    let dedup = DedupCache::new(100, Duration::from_secs(3600));
    let mut runner = WatchRunner::with_dedup(dedup).channel_capacity(16);

    runner.register(make_spec("empty", 1), InMemorySource::new(vec![]));
    let mut rx = runner.run();

    let result = tokio::time::timeout(Duration::from_millis(1200), rx.recv()).await;
    assert!(result.is_err(), "no events expected from an empty source");
}
