//! Outcome telemetry — "revenue, not time" ROI tracking (v1.2.4).
//!
//! Agents call [`OutcomeRecorder::record`] to attribute business outcomes
//! (revenue, cost savings, hours saved, etc.) to a session / agent pair.
//! The admin-ui Outcomes pane aggregates these for the ROI dashboard.
//!
//! Layering mirrors the `UsageReader` pattern from `xiaoguai-api`:
//! - The trait lives here so route handlers remain storage-agnostic.
//! - [`InMemoryOutcomeRecorder`] is the in-process implementation for unit
//!   tests; it records calls so tests can assert without touching PG.
//! - `PgOutcomeRecorder` lives in `xiaoguai-core/src/outcomes_bridge.rs`
//!   (wired at runtime against the `agent_outcomes` table, migration 0012).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Error)]
pub enum OutcomeError {
    #[error("outcome backend: {0}")]
    Backend(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// The kind of outcome being attributed.
///
/// The six first-class variants cover the most common institutional-AI
/// value reporting categories.  `Custom` allows operators to define their
/// own.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    RevenueUsd,
    CostSavedUsd,
    HoursSaved,
    DealsClosed,
    TicketsResolved,
    Custom,
}

impl OutcomeKind {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RevenueUsd => "revenue_usd",
            Self::CostSavedUsd => "cost_saved_usd",
            Self::HoursSaved => "hours_saved",
            Self::DealsClosed => "deals_closed",
            Self::TicketsResolved => "tickets_resolved",
            Self::Custom => "custom",
        }
    }

    /// Parse the string representation used in the DB / wire.
    #[must_use]
    pub fn from_kind_str(s: &str) -> Option<Self> {
        Some(match s {
            "revenue_usd" => Self::RevenueUsd,
            "cost_saved_usd" => Self::CostSavedUsd,
            "hours_saved" => Self::HoursSaved,
            "deals_closed" => Self::DealsClosed,
            "tickets_resolved" => Self::TicketsResolved,
            "custom" => Self::Custom,
            _ => return None,
        })
    }
}

/// A single recorded outcome event. Mirrors the `agent_outcomes` row shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutcomeRecord {
    pub tenant_id: String,
    pub session_id: Option<String>,
    pub agent_name: String,
    pub kind: String,
    pub value: f64,
    pub unit: Option<String>,
    pub description: Option<String>,
    pub attributed_at: DateTime<Utc>,
    pub metadata: serde_json::Value,
}

/// Aggregated view of outcomes — returned by
/// [`OutcomeRecorder::aggregate`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Aggregate {
    pub sum: f64,
    pub count: u64,
    pub avg: f64,
}

impl Aggregate {
    #[must_use]
    fn from_values(values: &[f64]) -> Self {
        if values.is_empty() {
            return Self {
                sum: 0.0,
                count: 0,
                avg: 0.0,
            };
        }
        let sum: f64 = values.iter().sum();
        let count = u64::try_from(values.len()).unwrap_or(u64::MAX);
        #[allow(
            clippy::cast_precision_loss,
            reason = "count fits safely in f64 for avg calculation"
        )]
        let avg = sum / count as f64;
        Self { sum, count, avg }
    }
}

/// Time range filter used by [`OutcomeRecorder::aggregate`].
#[derive(Debug, Clone, Default)]
pub struct OutcomeRange {
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
}

impl OutcomeRange {
    /// Build from a short human string: `"24h"`, `"7d"`, `"30d"`.
    ///
    /// Returns an error on unrecognised input.
    pub fn from_shorthand(s: &str) -> Result<Self, OutcomeError> {
        let now = Utc::now();
        let hours: i64 = match s {
            "24h" => 24,
            "7d" => 7 * 24,
            "30d" => 30 * 24,
            other => {
                return Err(OutcomeError::InvalidArgument(format!(
                    "unknown range '{other}'; expected 24h | 7d | 30d"
                )));
            }
        };
        let since = now - chrono::Duration::hours(hours);
        Ok(Self {
            since: Some(since),
            until: Some(now),
        })
    }
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait OutcomeRecorder: Send + Sync {
    /// Record a single outcome attribution.
    ///
    /// `metadata` is arbitrary JSON context (e.g. CRM deal ID, ticket URL).
    #[allow(clippy::too_many_arguments)]
    async fn record(
        &self,
        tenant_id: &str,
        session_id: Option<&str>,
        agent_name: &str,
        kind: &str,
        value: f64,
        unit: Option<&str>,
        description: Option<&str>,
        metadata: serde_json::Value,
    ) -> Result<(), OutcomeError>;

    /// Aggregate outcome values for a tenant over a time range.
    ///
    /// Returns the sum, count, and average of `value` for every row whose
    /// `(tenant_id, kind)` pair matches and whose `attributed_at` falls
    /// within `range`.  `kind = None` returns cross-kind totals.
    async fn aggregate(
        &self,
        tenant_id: &str,
        kind: Option<&str>,
        range: OutcomeRange,
    ) -> Result<Aggregate, OutcomeError>;
}

// ---------------------------------------------------------------------------
// InMemoryOutcomeRecorder — for unit tests
// ---------------------------------------------------------------------------

/// In-memory [`OutcomeRecorder`] backed by a `Mutex<Vec<OutcomeRecord>>`.
///
/// Thread-safe via the inner mutex so it can be cloned into `Arc<...>` and
/// shared across async tasks in tests.
#[derive(Debug, Default, Clone)]
pub struct InMemoryOutcomeRecorder {
    records: Arc<Mutex<Vec<OutcomeRecord>>>,
}

impl InMemoryOutcomeRecorder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of all recorded outcomes — useful for assertions.
    ///
    /// # Panics
    /// Panics if the internal lock is poisoned.
    #[must_use]
    pub fn snapshot(&self) -> Vec<OutcomeRecord> {
        self.records.lock().unwrap().clone()
    }
}

#[async_trait]
impl OutcomeRecorder for InMemoryOutcomeRecorder {
    async fn record(
        &self,
        tenant_id: &str,
        session_id: Option<&str>,
        agent_name: &str,
        kind: &str,
        value: f64,
        unit: Option<&str>,
        description: Option<&str>,
        metadata: serde_json::Value,
    ) -> Result<(), OutcomeError> {
        if value < 0.0 {
            return Err(OutcomeError::InvalidArgument(
                "value must be non-negative".into(),
            ));
        }
        if kind.is_empty() {
            return Err(OutcomeError::InvalidArgument(
                "kind must not be empty".into(),
            ));
        }
        // Emit Prometheus metrics for outcomes recording.
        if let Some(ctr) = xiaoguai_observability::outcomes_recorded_total() {
            ctr.with_label_values(&[kind]).inc();
        }
        // Chain depth: derive from the metadata "chain_depth" field if present,
        // otherwise default to 1 (single-turn attribution).
        let chain_depth = metadata
            .get("chain_depth")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0);
        if let Some(hist) = xiaoguai_observability::outcomes_chain_depth() {
            hist.observe(chain_depth);
        }
        let rec = OutcomeRecord {
            tenant_id: tenant_id.to_owned(),
            session_id: session_id.map(ToOwned::to_owned),
            agent_name: agent_name.to_owned(),
            kind: kind.to_owned(),
            value,
            unit: unit.map(ToOwned::to_owned),
            description: description.map(ToOwned::to_owned),
            attributed_at: Utc::now(),
            metadata,
        };
        self.records.lock().unwrap().push(rec);
        Ok(())
    }

    async fn aggregate(
        &self,
        tenant_id: &str,
        kind: Option<&str>,
        range: OutcomeRange,
    ) -> Result<Aggregate, OutcomeError> {
        if let (Some(since), Some(until)) = (range.since, range.until) {
            if since > until {
                return Err(OutcomeError::InvalidArgument(
                    "since must be <= until".into(),
                ));
            }
        }
        let records = self.records.lock().unwrap();
        let values: Vec<f64> = records
            .iter()
            .filter(|r| r.tenant_id == tenant_id)
            .filter(|r| kind.is_none_or(|k| r.kind == k))
            .filter(|r| range.since.is_none_or(|s| r.attributed_at >= s))
            .filter(|r| range.until.is_none_or(|u| r.attributed_at <= u))
            .map(|r| r.value)
            .collect();
        Ok(Aggregate::from_values(&values))
    }
}

// ---------------------------------------------------------------------------
// Per-kind summary helper (used by the summary endpoint)
// ---------------------------------------------------------------------------

/// Aggregated outcomes broken down by kind — returned by the
/// `GET /v1/outcomes/summary` handler for the ROI dashboard cards.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OutcomeSummary {
    /// Each entry is `(kind_string, Aggregate)`, sorted by kind name.
    pub by_kind: BTreeMap<String, Aggregate>,
}

impl OutcomeSummary {
    /// Build a summary from a flat slice of records already filtered to the
    /// desired tenant + range.  Used by both the in-memory path (tests) and
    /// by production code that pre-fetches from PG.
    #[must_use]
    pub fn from_records(records: &[OutcomeRecord]) -> Self {
        let mut by_kind: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for r in records {
            by_kind.entry(r.kind.clone()).or_default().push(r.value);
        }
        Self {
            by_kind: by_kind
                .into_iter()
                .map(|(k, vs)| (k, Aggregate::from_values(&vs)))
                .collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// Timeseries helper (daily buckets)
// ---------------------------------------------------------------------------

/// One daily bucket for `GET /v1/outcomes/timeseries`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutcomeDay {
    /// ISO-8601 date string `YYYY-MM-DD`.
    pub date: String,
    pub kind: String,
    pub sum: f64,
    pub count: u64,
}

/// Bucket a flat slice of records into daily `OutcomeDay` entries.
#[must_use]
pub fn timeseries(records: &[OutcomeRecord]) -> Vec<OutcomeDay> {
    let mut map: BTreeMap<(String, String), Vec<f64>> = BTreeMap::new();
    for r in records {
        let date = r.attributed_at.format("%Y-%m-%d").to_string();
        map.entry((date, r.kind.clone())).or_default().push(r.value);
    }
    map.into_iter()
        .map(|((date, kind), vs)| {
            let sum: f64 = vs.iter().sum();
            OutcomeDay {
                date,
                kind,
                sum,
                count: vs.len() as u64,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    type EntrySpec<'a> = (
        &'a str,
        Option<&'a str>,
        &'a str,
        &'a str,
        f64,
        Option<&'a str>,
    );

    async fn recorder_with_entries(entries: &[EntrySpec<'_>]) -> InMemoryOutcomeRecorder {
        let r = InMemoryOutcomeRecorder::new();
        for (tenant, session, agent, kind, value, unit) in entries {
            r.record(
                tenant,
                *session,
                agent,
                kind,
                *value,
                *unit,
                None,
                serde_json::Value::Null,
            )
            .await
            .unwrap();
        }
        r
    }

    #[tokio::test]
    async fn record_and_retrieve_snapshot() {
        let r = InMemoryOutcomeRecorder::new();
        r.record(
            "t1",
            Some("s1"),
            "bot",
            "revenue_usd",
            100.0,
            Some("usd"),
            Some("deal"),
            serde_json::json!({"deal_id": "D1"}),
        )
        .await
        .unwrap();
        let snap = r.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].tenant_id, "t1");
        assert_eq!(snap[0].kind, "revenue_usd");
        assert!((snap[0].value - 100.0).abs() < f64::EPSILON);
        assert_eq!(snap[0].metadata["deal_id"], "D1");
    }

    #[tokio::test]
    async fn aggregate_sum_count_avg() {
        let r = recorder_with_entries(&[
            ("ten", None, "bot", "hours_saved", 2.0, Some("hours")),
            ("ten", None, "bot", "hours_saved", 4.0, Some("hours")),
            ("ten", None, "bot", "hours_saved", 6.0, Some("hours")),
        ])
        .await;
        let agg = r
            .aggregate("ten", Some("hours_saved"), OutcomeRange::default())
            .await
            .unwrap();
        assert!((agg.sum - 12.0).abs() < f64::EPSILON);
        assert_eq!(agg.count, 3);
        assert!((agg.avg - 4.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn aggregate_filters_by_tenant() {
        let r = recorder_with_entries(&[
            ("ten_a", None, "bot", "deals_closed", 1.0, None),
            ("ten_b", None, "bot", "deals_closed", 99.0, None),
        ])
        .await;
        let agg = r
            .aggregate("ten_a", Some("deals_closed"), OutcomeRange::default())
            .await
            .unwrap();
        assert!((agg.sum - 1.0).abs() < f64::EPSILON);
        assert_eq!(agg.count, 1);
    }

    #[tokio::test]
    async fn aggregate_cross_kind_totals() {
        let r = recorder_with_entries(&[
            ("ten", None, "bot", "revenue_usd", 50.0, None),
            ("ten", None, "bot", "cost_saved_usd", 30.0, None),
            ("ten", None, "bot", "hours_saved", 10.0, None),
        ])
        .await;
        let agg = r
            .aggregate("ten", None, OutcomeRange::default())
            .await
            .unwrap();
        assert!((agg.sum - 90.0).abs() < f64::EPSILON);
        assert_eq!(agg.count, 3);
    }

    #[tokio::test]
    async fn aggregate_empty_returns_zero() {
        let r = InMemoryOutcomeRecorder::new();
        let agg = r
            .aggregate("nobody", Some("revenue_usd"), OutcomeRange::default())
            .await
            .unwrap();
        assert!(agg.sum.abs() < f64::EPSILON);
        assert_eq!(agg.count, 0);
        assert!(agg.avg.abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn aggregate_rejects_inverted_range() {
        let r = InMemoryOutcomeRecorder::new();
        let later = Utc.with_ymd_and_hms(2026, 5, 25, 0, 0, 0).unwrap();
        let earlier = Utc.with_ymd_and_hms(2026, 5, 24, 0, 0, 0).unwrap();
        let err = r
            .aggregate(
                "ten",
                None,
                OutcomeRange {
                    since: Some(later),
                    until: Some(earlier),
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, OutcomeError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn record_rejects_negative_value() {
        let r = InMemoryOutcomeRecorder::new();
        let err = r
            .record(
                "t1",
                None,
                "bot",
                "revenue_usd",
                -1.0,
                None,
                None,
                serde_json::Value::Null,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, OutcomeError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn record_rejects_empty_kind() {
        let r = InMemoryOutcomeRecorder::new();
        let err = r
            .record(
                "t1",
                None,
                "bot",
                "",
                1.0,
                None,
                None,
                serde_json::Value::Null,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, OutcomeError::InvalidArgument(_)));
    }

    #[test]
    fn outcome_kind_round_trip() {
        for kind in [
            OutcomeKind::RevenueUsd,
            OutcomeKind::CostSavedUsd,
            OutcomeKind::HoursSaved,
            OutcomeKind::DealsClosed,
            OutcomeKind::TicketsResolved,
            OutcomeKind::Custom,
        ] {
            let s = kind.as_str();
            assert_eq!(OutcomeKind::from_kind_str(s), Some(kind));
        }
        assert!(OutcomeKind::from_kind_str("unknown_kind").is_none());
    }

    #[test]
    fn outcome_summary_from_records() {
        let records = vec![
            OutcomeRecord {
                tenant_id: "t".into(),
                session_id: None,
                agent_name: "bot".into(),
                kind: "revenue_usd".into(),
                value: 100.0,
                unit: None,
                description: None,
                attributed_at: Utc::now(),
                metadata: serde_json::Value::Null,
            },
            OutcomeRecord {
                tenant_id: "t".into(),
                session_id: None,
                agent_name: "bot".into(),
                kind: "revenue_usd".into(),
                value: 200.0,
                unit: None,
                description: None,
                attributed_at: Utc::now(),
                metadata: serde_json::Value::Null,
            },
            OutcomeRecord {
                tenant_id: "t".into(),
                session_id: None,
                agent_name: "bot".into(),
                kind: "hours_saved".into(),
                value: 4.0,
                unit: None,
                description: None,
                attributed_at: Utc::now(),
                metadata: serde_json::Value::Null,
            },
        ];
        let summary = OutcomeSummary::from_records(&records);
        let rev = &summary.by_kind["revenue_usd"];
        assert!((rev.sum - 300.0).abs() < f64::EPSILON);
        assert_eq!(rev.count, 2);
        assert!((rev.avg - 150.0).abs() < f64::EPSILON);
        let hrs = &summary.by_kind["hours_saved"];
        assert!((hrs.sum - 4.0).abs() < f64::EPSILON);
    }

    #[test]
    fn timeseries_buckets_by_day_and_kind() {
        let d1 = Utc.with_ymd_and_hms(2026, 5, 20, 1, 0, 0).unwrap();
        let d2 = Utc.with_ymd_and_hms(2026, 5, 21, 9, 0, 0).unwrap();
        let make = |ts: DateTime<Utc>, kind: &str, value: f64| OutcomeRecord {
            tenant_id: "t".into(),
            session_id: None,
            agent_name: "bot".into(),
            kind: kind.to_owned(),
            value,
            unit: None,
            description: None,
            attributed_at: ts,
            metadata: serde_json::Value::Null,
        };
        let records = vec![
            make(d1, "revenue_usd", 50.0),
            make(d1, "revenue_usd", 50.0),
            make(d2, "hours_saved", 3.0),
        ];
        let ts = timeseries(&records);
        assert_eq!(ts.len(), 2);
        let rev = ts.iter().find(|d| d.kind == "revenue_usd").unwrap();
        assert_eq!(rev.date, "2026-05-20");
        assert!((rev.sum - 100.0).abs() < f64::EPSILON);
        assert_eq!(rev.count, 2);
        let hrs = ts.iter().find(|d| d.kind == "hours_saved").unwrap();
        assert_eq!(hrs.date, "2026-05-21");
    }

    #[test]
    fn range_shorthand_24h() {
        let r = OutcomeRange::from_shorthand("24h").unwrap();
        assert!(r.since.is_some());
        assert!(r.until.is_some());
    }

    #[test]
    fn range_shorthand_unknown_returns_error() {
        let err = OutcomeRange::from_shorthand("99y").unwrap_err();
        assert!(matches!(err, OutcomeError::InvalidArgument(_)));
    }
}
