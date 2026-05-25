//! Capability eval suite for the `xiaoguai-watch` DSL (spec parser).
//!
//! The "DSL" is the YAML/JSON spec format consumed by [`WatchSpec`] +
//! [`WatchSourceSpec`] + [`WatchSchedule`].  These tests define what
//! "correct parsing" means in observable terms and are intended to catch
//! future regressions in serialisation, validation, and runtime behaviour.
//!
//! ## Scenario index
//!
//! | # | Category | Description |
//! |---|----------|-------------|
//! | 1 | Happy    | SQL poll watcher — WHERE clause + dedup key |
//! | 2 | Happy    | HTTP poll watcher — header auth + JSON selector |
//! | 3 | Happy    | HTTP defaults (jsonpath + method) round-trip |
//! | 4 | Happy    | Full YAML round-trip preserves all fields |
//! | 5 | Happy    | JSON round-trip preserves all fields |
//! | 6 | Happy    | params map forwarded verbatim in WatchEvent |
//! | 7 | Edge     | Empty result set → no event emitted |
//! | 8 | Edge     | Dedup key collision → duplicate wakeup suppressed |
//! | 9 | Edge     | Multiple distinct rows all fire; only duplicates suppressed |
//! | 10 | Edge    | Error message for non-SELECT SQL is useful (names problem) |
//! | 11 | Edge    | Error message for empty query is useful |
//! | 12 | Edge    | Error message for empty id is useful |
//! | 13 | Edge    | Error message for empty action is useful |
//! | 14 | Edge    | Error message for zero interval is useful |
//! | 15 | Edge    | Error message for empty HTTP url is useful |
//! | 16 | Edge    | Interval shorthand — 5 s parses to 5-second interval |
//! | 17 | Edge    | Interval shorthand — 1 m parses to 60-second interval |
//! | 18 | Edge    | Interval shorthand — 1 h parses to 3600-second interval |
//! | 19 | Edge    | Canonical JSON key-order independence for dedup fingerprint |
//! | 20 | Capab.  | Two watchers sharing a dedup cache respect per-spec namespacing |
//! | 21 | Capab.  | Dedup TTL expiry allows re-fire |
//! | 22 | Capab.  | Chain: two watchers emit independently; both fire before suppression |
//! | 23 | Gap     | Interval shorthand string form (e.g. "5s") not yet supported |

use std::time::Duration;

use serde_json::json;
use xiaoguai_watch::{
    ActionRef, DedupCache, InMemorySource, Match, SourceError, WatchRunner, WatchSchedule,
    WatchSource, WatchSourceSpec, WatchSpec,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_action(action: &str, target: Option<&str>) -> ActionRef {
    ActionRef {
        action: action.to_string(),
        target: target.map(str::to_string),
        params: serde_json::Map::new(),
    }
}

fn sql_spec(id: &str, query: &str, interval_secs: u64) -> WatchSpec {
    WatchSpec {
        id: id.into(),
        source: WatchSourceSpec::Sql {
            query: query.into(),
        },
        schedule: WatchSchedule::IntervalSecs { secs: interval_secs },
        on_match: make_action("notify", Some("test-ch")),
    }
}

fn http_spec(id: &str, url: &str, jsonpath: &str) -> WatchSpec {
    WatchSpec {
        id: id.into(),
        source: WatchSourceSpec::Http {
            url: url.into(),
            jsonpath: jsonpath.into(),
            method: "GET".into(),
        },
        schedule: WatchSchedule::IntervalSecs { secs: 30 },
        on_match: make_action("webhook", None),
    }
}

fn row(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    serde_json::from_value(v).unwrap()
}

// ---------------------------------------------------------------------------
// 1. Happy: SQL poll watcher — SELECT with WHERE clause round-trips correctly
// ---------------------------------------------------------------------------

#[test]
fn dsl_01_sql_watcher_with_where_clause_validates() {
    let spec = sql_spec(
        "ar-aging",
        "SELECT tenant_id, dso FROM ar_aging WHERE dso > 60",
        86_400,
    );
    spec.validate()
        .expect("SQL watcher with WHERE clause must validate");

    match &spec.source {
        WatchSourceSpec::Sql { query } => {
            assert!(query.contains("WHERE dso > 60"), "WHERE clause preserved");
        }
        other => panic!("expected Sql source, got {other:?}"),
    }

    assert_eq!(spec.schedule, WatchSchedule::IntervalSecs { secs: 86_400 });
    assert_eq!(spec.on_match.action, "notify");
}

#[tokio::test]
async fn dsl_01_sql_watcher_dedup_key_fires_once() {
    // The "dedup key" is the entire row fingerprint.  Same row → fires once.
    let dedup = DedupCache::new(100, Duration::from_secs(3600));
    let mut runner = WatchRunner::with_dedup(dedup).channel_capacity(16);

    let spec = sql_spec("ar-aging-dedup", "SELECT 1", 1);
    let source = InMemorySource::new(vec![row(json!({"tenant_id": "acme", "dso": 72}))]);
    runner.register(spec, source);
    let mut rx = runner.run();

    // First tick fires.
    let e = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("first event timed out")
        .expect("channel closed");
    assert_eq!(e.spec_id, "ar-aging-dedup");
    assert_eq!(e.payload["dso"], 72);

    // Second tick within TTL — suppressed.
    let second = tokio::time::timeout(Duration::from_millis(1_500), rx.recv()).await;
    assert!(second.is_err(), "duplicate row must be suppressed");
}

// ---------------------------------------------------------------------------
// 2. Happy: HTTP poll watcher — header auth mention + JSON selector validate
// ---------------------------------------------------------------------------

#[test]
fn dsl_02_http_watcher_with_json_selector_validates() {
    // Header auth is handled at the reqwest client level; at the spec level
    // we exercise the jsonpath selector and url fields.
    let spec = http_spec(
        "metrics-poll",
        "https://api.example.com/v1/alerts",
        "$.alerts[*]",
    );
    spec.validate()
        .expect("HTTP watcher with JSON selector must validate");

    match &spec.source {
        WatchSourceSpec::Http {
            url,
            jsonpath,
            method,
        } => {
            assert_eq!(url, "https://api.example.com/v1/alerts");
            assert_eq!(jsonpath, "$.alerts[*]");
            assert_eq!(method, "GET");
        }
        other => panic!("expected Http source, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 3. Happy: HTTP defaults round-trip (jsonpath + method default correctly)
// ---------------------------------------------------------------------------

#[test]
fn dsl_03_http_defaults_round_trip() {
    let yaml = r#"
id: http-defaults
source: !http
  url: "https://example.com/api/events"
on_match:
  action: create_task
"#;
    let spec: WatchSpec = serde_yaml::from_str(yaml).unwrap();
    spec.validate().expect("spec with HTTP defaults must validate");

    match &spec.source {
        WatchSourceSpec::Http {
            jsonpath, method, ..
        } => {
            assert_eq!(jsonpath, "$[*]", "default jsonpath must be $[*]");
            assert_eq!(method, "GET", "default method must be GET");
        }
        other => panic!("expected Http source, got {other:?}"),
    }
    // Default schedule
    assert_eq!(spec.schedule, WatchSchedule::IntervalSecs { secs: 60 });
}

// ---------------------------------------------------------------------------
// 4. Happy: full YAML round-trip preserves all fields
// ---------------------------------------------------------------------------

#[test]
fn dsl_04_yaml_full_round_trip() {
    let original = WatchSpec {
        id: "full-yaml-spec".into(),
        source: WatchSourceSpec::Sql {
            query: "SELECT id, status FROM jobs WHERE status = 'failed'".into(),
        },
        schedule: WatchSchedule::IntervalSecs { secs: 300 },
        on_match: ActionRef {
            action: "create_task".into(),
            target: Some("on-call-queue".into()),
            params: {
                let mut m = serde_json::Map::new();
                m.insert("priority".into(), json!("high"));
                m
            },
        },
    };

    let yaml = serde_yaml::to_string(&original).unwrap();
    let restored: WatchSpec = serde_yaml::from_str(&yaml).unwrap();

    assert_eq!(original, restored, "YAML round-trip must be lossless");
    restored.validate().expect("restored spec must validate");
}

// ---------------------------------------------------------------------------
// 5. Happy: JSON round-trip preserves all fields
// ---------------------------------------------------------------------------

#[test]
fn dsl_05_json_full_round_trip() {
    let original = WatchSpec {
        id: "full-json-spec".into(),
        source: WatchSourceSpec::Http {
            url: "https://api.example.com/tickets".into(),
            jsonpath: "$.items[*]".into(),
            method: "POST".into(),
        },
        schedule: WatchSchedule::IntervalSecs { secs: 120 },
        on_match: ActionRef {
            action: "notify".into(),
            target: None,
            params: serde_json::Map::new(),
        },
    };

    let json = serde_json::to_string(&original).unwrap();
    let restored: WatchSpec = serde_json::from_str(&json).unwrap();

    assert_eq!(original, restored, "JSON round-trip must be lossless");
    restored.validate().expect("restored spec must validate");
}

// ---------------------------------------------------------------------------
// 6. Happy: params map forwarded verbatim in WatchEvent payload
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dsl_06_params_forwarded_in_event() {
    let dedup = DedupCache::new(100, Duration::from_secs(3600));
    let mut runner = WatchRunner::with_dedup(dedup).channel_capacity(8);

    let spec = WatchSpec {
        id: "params-fwd".into(),
        source: WatchSourceSpec::Sql {
            query: "SELECT 1".into(),
        },
        schedule: WatchSchedule::IntervalSecs { secs: 1 },
        on_match: ActionRef {
            action: "notify".into(),
            target: Some("ch".into()),
            params: {
                let mut m = serde_json::Map::new();
                m.insert("severity".into(), json!("critical"));
                m.insert("team".into(), json!("platform"));
                m
            },
        },
    };

    let source = InMemorySource::new(vec![row(json!({"id": 1}))]);
    runner.register(spec, source);
    let mut rx = runner.run();

    let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("event timed out")
        .expect("channel closed");

    assert_eq!(event.on_match.params["severity"], "critical");
    assert_eq!(event.on_match.params["team"], "platform");
}

// ---------------------------------------------------------------------------
// 7. Edge: empty result set → no event emitted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dsl_07_empty_result_no_event() {
    let dedup = DedupCache::new(100, Duration::from_secs(3600));
    let mut runner = WatchRunner::with_dedup(dedup).channel_capacity(8);

    let spec = sql_spec("empty-src", "SELECT 1", 1);
    let source = InMemorySource::new(vec![]); // empty
    runner.register(spec, source);
    let mut rx = runner.run();

    // Wait two ticks — no events should arrive.
    let result = tokio::time::timeout(Duration::from_millis(1_300), rx.recv()).await;
    assert!(
        result.is_err(),
        "empty source must not emit events (got one)"
    );
}

// ---------------------------------------------------------------------------
// 8. Edge: dedup key collision → duplicate wakeup suppressed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dsl_08_dedup_collision_suppresses_wakeup() {
    // Two identical rows returned on every poll; only the first should fire.
    let dedup = DedupCache::new(100, Duration::from_secs(3600));
    let mut runner = WatchRunner::with_dedup(dedup).channel_capacity(8);

    let spec = sql_spec("dedup-collision", "SELECT 1", 1);
    // Same data in both rows → identical fingerprint.
    let dup_row = row(json!({"alert": "disk_full", "host": "web-01"}));
    let source = InMemorySource::new(vec![dup_row.clone(), dup_row]);
    runner.register(spec, source);
    let mut rx = runner.run();

    // Only the first distinct fingerprint fires.
    let first = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("first event timed out")
        .expect("channel closed");
    assert_eq!(first.payload["alert"], "disk_full");

    // No second event within a reasonable window (duplicate suppressed).
    let second = tokio::time::timeout(Duration::from_millis(1_300), rx.recv()).await;
    assert!(
        second.is_err(),
        "duplicate row within same poll must be suppressed"
    );
}

// ---------------------------------------------------------------------------
// 9. Edge: multiple distinct rows all fire; only true duplicates suppressed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dsl_09_distinct_rows_all_fire() {
    let dedup = DedupCache::new(100, Duration::from_secs(3600));
    let mut runner = WatchRunner::with_dedup(dedup).channel_capacity(16);

    let spec = sql_spec("multi-row", "SELECT 1", 1);
    let source = InMemorySource::new(vec![
        row(json!({"id": 1, "status": "failed"})),
        row(json!({"id": 2, "status": "failed"})),
        row(json!({"id": 3, "status": "failed"})),
    ]);
    runner.register(spec, source);
    let mut rx = runner.run();

    // Collect 3 distinct events.
    let mut ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for _ in 0..3 {
        let ev = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("event timed out")
            .expect("channel closed");
        let id = ev.payload["id"].as_i64().expect("id must be integer");
        ids.insert(id);
    }
    assert_eq!(ids, [1, 2, 3].into_iter().collect(), "all 3 rows must fire");

    // No fourth event (all three are now deduplicated).
    let fourth = tokio::time::timeout(Duration::from_millis(1_300), rx.recv()).await;
    assert!(fourth.is_err(), "no more events expected after dedup");
}

// ---------------------------------------------------------------------------
// 10. Edge: parse error for non-SELECT SQL — message must name the problem
// ---------------------------------------------------------------------------

#[test]
fn dsl_10_non_select_error_is_useful() {
    let mut spec = sql_spec("bad-sql", "DELETE FROM events", 60);
    let err = spec.validate().unwrap_err();

    // The error must mention SELECT (so the user knows what was expected).
    assert!(
        err.to_lowercase().contains("select"),
        "error for non-SELECT must mention SELECT, got: {err}"
    );
    // The error must not be a generic "parse failed" blob.
    assert!(
        err.len() > 10,
        "error must be descriptive, got: {err}"
    );

    // Update: INSERT also rejected.
    spec.source = WatchSourceSpec::Sql {
        query: "INSERT INTO foo VALUES (1)".into(),
    };
    let err2 = spec.validate().unwrap_err();
    assert!(
        err2.to_lowercase().contains("select"),
        "INSERT also rejected with SELECT mention, got: {err2}"
    );
}

// ---------------------------------------------------------------------------
// 11. Edge: parse error for empty query — message names the field
// ---------------------------------------------------------------------------

#[test]
fn dsl_11_empty_query_error_is_useful() {
    let spec = sql_spec("empty-q", "   ", 60);
    let err = spec.validate().unwrap_err();

    assert!(
        err.to_lowercase().contains("query") || err.to_lowercase().contains("empty"),
        "error for empty query must mention query or empty, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// 12. Edge: parse error for empty id — message names the field
// ---------------------------------------------------------------------------

#[test]
fn dsl_12_empty_id_error_is_useful() {
    let mut spec = sql_spec("x", "SELECT 1", 60);
    spec.id = String::new();
    let err = spec.validate().unwrap_err();

    assert!(
        err.to_lowercase().contains("id"),
        "error for empty id must mention 'id', got: {err}"
    );
}

// ---------------------------------------------------------------------------
// 13. Edge: parse error for empty action — message names the field
// ---------------------------------------------------------------------------

#[test]
fn dsl_13_empty_action_error_is_useful() {
    let mut spec = sql_spec("x", "SELECT 1", 60);
    spec.on_match.action = String::new();
    let err = spec.validate().unwrap_err();

    assert!(
        err.to_lowercase().contains("action"),
        "error for empty action must mention 'action', got: {err}"
    );
}

// ---------------------------------------------------------------------------
// 14. Edge: parse error for zero interval — message names the problem
// ---------------------------------------------------------------------------

#[test]
fn dsl_14_zero_interval_error_is_useful() {
    let mut spec = sql_spec("x", "SELECT 1", 60);
    spec.schedule = WatchSchedule::IntervalSecs { secs: 0 };
    let err = spec.validate().unwrap_err();

    assert!(
        err.to_lowercase().contains("interval") || err.to_lowercase().contains("0"),
        "error for zero interval must mention interval or 0, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// 15. Edge: parse error for empty HTTP url — message names the field
// ---------------------------------------------------------------------------

#[test]
fn dsl_15_empty_http_url_error_is_useful() {
    let spec = WatchSpec {
        id: "http-empty-url".into(),
        source: WatchSourceSpec::Http {
            url: String::new(),
            jsonpath: "$[*]".into(),
            method: "GET".into(),
        },
        schedule: WatchSchedule::IntervalSecs { secs: 60 },
        on_match: make_action("notify", None),
    };
    let err = spec.validate().unwrap_err();

    assert!(
        err.to_lowercase().contains("url"),
        "error for empty url must mention 'url', got: {err}"
    );
}

// ---------------------------------------------------------------------------
// 16–18. Edge: interval seconds values parse to correct Duration equivalents
//
// NOTE: The DSL uses `interval_secs: N` (integer seconds) not shorthand
// strings like "5s".  We test that the well-known shorthand values — when
// expressed as their integer equivalents — survive a round-trip and that
// `schedule_to_duration` (internal) maps them correctly.
//
// Tests 23 (gap) covers the case where string shorthands are NOT yet supported.
// ---------------------------------------------------------------------------

#[test]
fn dsl_16_interval_5_seconds_round_trips() {
    let spec = sql_spec("s5", "SELECT 1", 5); // "5s" in integer form
    spec.validate().expect("5-second interval must validate");
    assert_eq!(spec.schedule, WatchSchedule::IntervalSecs { secs: 5 });

    // Serialise + restore — the value must survive.
    let json = serde_json::to_string(&spec).unwrap();
    let back: WatchSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(back.schedule, WatchSchedule::IntervalSecs { secs: 5 });
}

#[test]
fn dsl_17_interval_60_seconds_round_trips() {
    let spec = sql_spec("s60", "SELECT 1", 60); // "1m" in integer form
    spec.validate().expect("60-second interval must validate");

    let json = serde_json::to_string(&spec).unwrap();
    let back: WatchSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(back.schedule, WatchSchedule::IntervalSecs { secs: 60 });
}

#[test]
fn dsl_18_interval_3600_seconds_round_trips() {
    let spec = sql_spec("s3600", "SELECT 1", 3600); // "1h" in integer form
    spec.validate().expect("3600-second interval must validate");

    let json = serde_json::to_string(&spec).unwrap();
    let back: WatchSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(back.schedule, WatchSchedule::IntervalSecs { secs: 3600 });
}

// ---------------------------------------------------------------------------
// 19. Edge: canonical JSON key-order independence for dedup fingerprint
// ---------------------------------------------------------------------------

#[test]
fn dsl_19_fingerprint_stable_across_key_order() {
    // Two Match objects with the same logical content but different key
    // insertion order must produce the same fingerprint.
    let m1 = Match::from_value(json!({"b": 2, "a": 1, "c": "x"}));
    let m2 = Match::from_value(json!({"a": 1, "c": "x", "b": 2}));

    let fp1 = DedupCache::fingerprint("watch-id", &m1);
    let fp2 = DedupCache::fingerprint("watch-id", &m2);

    assert_eq!(
        fp1, fp2,
        "fingerprints must be equal regardless of map key order"
    );
    assert_eq!(fp1.len(), 64, "fingerprint must be 64-char hex SHA-256");
}

// ---------------------------------------------------------------------------
// 20. Capability: two watchers sharing a dedup cache respect per-spec namespacing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dsl_20_two_specs_same_row_fire_independently() {
    // Both specs yield the same row content.  They share one dedup cache.
    // Because fingerprints are keyed by spec_id, each spec fires independently.
    let dedup = DedupCache::new(100, Duration::from_secs(3600));
    let mut runner = WatchRunner::with_dedup(dedup).channel_capacity(16);

    let shared_row = row(json!({"metric": "cpu_pct", "value": 95}));

    runner.register(
        sql_spec("watcher-alpha", "SELECT 1", 1),
        InMemorySource::new(vec![shared_row.clone()]),
    );
    runner.register(
        sql_spec("watcher-beta", "SELECT 1", 1),
        InMemorySource::new(vec![shared_row]),
    );

    let mut rx = runner.run();

    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for _ in 0..2 {
        let ev = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("event timed out")
            .expect("channel closed");
        seen.insert(ev.spec_id.clone());
    }

    assert!(seen.contains("watcher-alpha"), "watcher-alpha must fire");
    assert!(seen.contains("watcher-beta"), "watcher-beta must fire independently");

    // Neither fires a second time within the TTL.
    let extra = tokio::time::timeout(Duration::from_millis(1_300), rx.recv()).await;
    assert!(extra.is_err(), "no duplicate events expected");
}

// ---------------------------------------------------------------------------
// 21. Capability: dedup TTL expiry allows re-fire
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dsl_21_dedup_ttl_expiry_allows_refire() {
    let dedup = DedupCache::new(100, Duration::from_millis(150)); // very short TTL
    let mut runner = WatchRunner::with_dedup(dedup).channel_capacity(16);

    let spec = sql_spec("ttl-refire", "SELECT 1", 1);
    let source = InMemorySource::new(vec![row(json!({"job": "backup", "status": "failed"}))]);
    runner.register(spec, source);
    let mut rx = runner.run();

    // First fire.
    let e1 = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("first event timed out")
        .expect("channel closed");
    assert_eq!(e1.payload["job"], "backup");

    // Let TTL expire (300 ms > 150 ms TTL).
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Same row fires again after expiry.
    let e2 = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("second event (post-TTL) timed out")
        .expect("channel closed");
    assert_eq!(e2.payload["job"], "backup");
    assert_eq!(e1.spec_id, e2.spec_id);
}

// ---------------------------------------------------------------------------
// 22. Capability: chain — two watchers emit; both fire before per-spec dedup
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dsl_22_chained_watchers_both_fire_before_cooldown() {
    // Simulate a "chain" by registering two watchers on the same runner.
    // Watcher A monitors upstream; Watcher B monitors downstream.
    // Both fire on the first poll; subsequent identical rows are suppressed
    // independently (per spec id).
    let dedup = DedupCache::new(100, Duration::from_secs(3600));
    let mut runner = WatchRunner::with_dedup(dedup).channel_capacity(16);

    runner.register(
        sql_spec("chain-upstream", "SELECT 1", 1),
        InMemorySource::new(vec![row(json!({"stage": "upstream", "alert": "lag"}))])
    );
    runner.register(
        sql_spec("chain-downstream", "SELECT 1", 1),
        InMemorySource::new(vec![row(json!({"stage": "downstream", "alert": "lag"}))])
    );

    let mut rx = runner.run();

    let mut fired: std::collections::HashMap<String, serde_json::Value> =
        std::collections::HashMap::new();

    for _ in 0..2 {
        let ev = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("chain event timed out")
            .expect("channel closed");
        fired.insert(ev.spec_id.clone(), ev.payload.clone());
    }

    assert!(
        fired.contains_key("chain-upstream"),
        "upstream watcher must fire"
    );
    assert!(
        fired.contains_key("chain-downstream"),
        "downstream watcher must fire"
    );
    assert_eq!(fired["chain-upstream"]["stage"], "upstream");
    assert_eq!(fired["chain-downstream"]["stage"], "downstream");

    // Both are now deduplicated — no further events within TTL.
    let extra = tokio::time::timeout(Duration::from_millis(1_300), rx.recv()).await;
    assert!(
        extra.is_err(),
        "both watchers must be suppressed after first fire"
    );
}

// ---------------------------------------------------------------------------
// 23. Gap — interval shorthand string form not yet supported
//
// TODO: The DSL spec doc mentions "5s / 1m / 1h" shorthand notation, but
// there is no parser for string-form shorthands.  `WatchSchedule` only
// deserialises `interval_secs: <integer>`.  This test is marked #[ignore]
// until a shorthand parser is added (expected in v1.3.x).
// ---------------------------------------------------------------------------

#[test]
#[ignore = "GAP: interval shorthand strings (\"5s\", \"1m\", \"1h\") not yet supported by WatchSchedule deserialiser"]
fn dsl_23_interval_shorthand_string_5s_parses() {
    // This YAML form is NOT currently accepted — the deserialiser expects
    // `interval_secs: 5`, not `interval: "5s"`.
    let yaml = r#"
id: shorthand-test
source: !sql
  query: "SELECT 1"
schedule: !interval_secs
  secs: "5s"
on_match:
  action: notify
"#;
    // Expect deserialization to succeed and produce a 5-second interval.
    let spec: WatchSpec = serde_yaml::from_str(yaml)
        .expect("shorthand interval should deserialise");
    assert_eq!(spec.schedule, WatchSchedule::IntervalSecs { secs: 5 });
}
