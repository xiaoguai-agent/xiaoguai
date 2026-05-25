//! v1.2.4 — Outcome telemetry API surface.
//!
//! Agents call `POST /v1/outcomes` to attribute business value (revenue, cost
//! savings, hours, etc.) to a session.  The admin-ui Outcomes pane drives:
//!   - `GET /v1/outcomes/summary?tenant=X&range=24h|7d|30d` — ROI cards
//!   - `GET /v1/outcomes/timeseries?tenant=X&range=...&kind=...` — bar chart
//!
//! Layering mirrors `UsageReader`:
//!   - The [`OutcomeWriter`] + [`OutcomesReader`] traits live here so route
//!     handlers are storage-agnostic.
//!   - [`InMemoryOutcomesBackend`] is the in-process implementation used by
//!     unit tests.
//!   - The PG implementation lives in `xiaoguai-core/src/outcomes_bridge.rs`.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use xiaoguai_audit::outcomes::{
    Aggregate, OutcomeDay, OutcomeError, OutcomeRange, OutcomeRecord, OutcomeRecorder,
    OutcomeSummary,
};

pub use xiaoguai_audit::outcomes::{InMemoryOutcomeRecorder, OutcomeKind};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Error)]
pub enum OutcomesApiError {
    #[error("outcomes backend: {0}")]
    Backend(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

impl From<OutcomeError> for OutcomesApiError {
    fn from(e: OutcomeError) -> Self {
        match e {
            OutcomeError::Backend(s) => Self::Backend(s),
            OutcomeError::InvalidArgument(s) => Self::InvalidArgument(s),
        }
    }
}

// ---------------------------------------------------------------------------
// Wire types (POST /v1/outcomes body)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordOutcomeRequest {
    pub tenant_id: String,
    pub session_id: Option<String>,
    pub agent_name: String,
    /// One of the well-known kinds or `"custom"`.
    pub kind: String,
    pub value: f64,
    pub unit: Option<String>,
    pub description: Option<String>,
    #[serde(default = "serde_json::Value::default")]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordOutcomeResponse {
    pub ok: bool,
}

// ---------------------------------------------------------------------------
// Summary response
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutcomesSummaryResponse {
    pub tenant_id: String,
    pub range: String,
    pub summary: OutcomeSummary,
}

// ---------------------------------------------------------------------------
// Timeseries response
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutcomesTimeseriesResponse {
    pub tenant_id: String,
    pub range: String,
    pub days: Vec<OutcomeDay>,
}

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// Write side — agents use this to record outcome attributions.
#[async_trait]
pub trait OutcomeWriter: Send + Sync {
    async fn record(&self, req: RecordOutcomeRequest) -> Result<(), OutcomesApiError>;
}

/// Read side — admin-ui uses this for the ROI dashboard.
#[async_trait]
pub trait OutcomesReader: Send + Sync {
    async fn summary(
        &self,
        tenant_id: &str,
        range: OutcomeRange,
    ) -> Result<OutcomeSummary, OutcomesApiError>;

    async fn timeseries(
        &self,
        tenant_id: &str,
        kind: Option<&str>,
        range: OutcomeRange,
    ) -> Result<Vec<OutcomeDay>, OutcomesApiError>;

    async fn aggregate(
        &self,
        tenant_id: &str,
        kind: Option<&str>,
        range: OutcomeRange,
    ) -> Result<Aggregate, OutcomesApiError>;
}

// ---------------------------------------------------------------------------
// In-memory backend (test / dev)
// ---------------------------------------------------------------------------

/// Combined writer + reader backed by [`InMemoryOutcomeRecorder`].
/// Both traits delegate to the same recorder so writes are immediately
/// visible to reads.
#[derive(Debug, Clone, Default)]
pub struct InMemoryOutcomesBackend {
    inner: Arc<InMemoryOutcomeRecorder>,
}

impl InMemoryOutcomesBackend {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<OutcomeRecord> {
        self.inner.snapshot()
    }
}

#[async_trait]
impl OutcomeWriter for InMemoryOutcomesBackend {
    async fn record(&self, req: RecordOutcomeRequest) -> Result<(), OutcomesApiError> {
        self.inner
            .record(
                &req.tenant_id,
                req.session_id.as_deref(),
                &req.agent_name,
                &req.kind,
                req.value,
                req.unit.as_deref(),
                req.description.as_deref(),
                req.metadata,
            )
            .await
            .map_err(OutcomesApiError::from)
    }
}

#[async_trait]
impl OutcomesReader for InMemoryOutcomesBackend {
    async fn summary(
        &self,
        tenant_id: &str,
        range: OutcomeRange,
    ) -> Result<OutcomeSummary, OutcomesApiError> {
        // Collect matching records and build summary locally.
        let all = self.inner.snapshot();
        let filtered: Vec<OutcomeRecord> = all
            .into_iter()
            .filter(|r| r.tenant_id == tenant_id)
            .filter(|r| range.since.map_or(true, |s| r.attributed_at >= s))
            .filter(|r| range.until.map_or(true, |u| r.attributed_at <= u))
            .collect();
        Ok(OutcomeSummary::from_records(&filtered))
    }

    async fn timeseries(
        &self,
        tenant_id: &str,
        kind: Option<&str>,
        range: OutcomeRange,
    ) -> Result<Vec<OutcomeDay>, OutcomesApiError> {
        let all = self.inner.snapshot();
        let filtered: Vec<OutcomeRecord> = all
            .into_iter()
            .filter(|r| r.tenant_id == tenant_id)
            .filter(|r| kind.map_or(true, |k| r.kind == k))
            .filter(|r| range.since.map_or(true, |s| r.attributed_at >= s))
            .filter(|r| range.until.map_or(true, |u| r.attributed_at <= u))
            .collect();
        Ok(xiaoguai_audit::timeseries(&filtered))
    }

    async fn aggregate(
        &self,
        tenant_id: &str,
        kind: Option<&str>,
        range: OutcomeRange,
    ) -> Result<Aggregate, OutcomesApiError> {
        self.inner
            .aggregate(tenant_id, kind, range)
            .await
            .map_err(OutcomesApiError::from)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn req(kind: &str, value: f64) -> RecordOutcomeRequest {
        RecordOutcomeRequest {
            tenant_id: "tenant_a".into(),
            session_id: Some("sess_1".into()),
            agent_name: "sales-bot".into(),
            kind: kind.to_owned(),
            value,
            unit: Some("usd".into()),
            description: Some("test record".into()),
            metadata: json!({}),
        }
    }

    #[tokio::test]
    async fn record_and_aggregate() {
        let b = InMemoryOutcomesBackend::new();
        b.record(req("revenue_usd", 500.0)).await.unwrap();
        b.record(req("revenue_usd", 300.0)).await.unwrap();
        let agg = b
            .aggregate("tenant_a", Some("revenue_usd"), OutcomeRange::default())
            .await
            .unwrap();
        assert!((agg.sum - 800.0).abs() < f64::EPSILON);
        assert_eq!(agg.count, 2);
        assert!((agg.avg - 400.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn summary_groups_by_kind() {
        let b = InMemoryOutcomesBackend::new();
        b.record(req("revenue_usd", 100.0)).await.unwrap();
        b.record(req("cost_saved_usd", 50.0)).await.unwrap();
        b.record(req("hours_saved", 8.0)).await.unwrap();
        let summary = b
            .summary("tenant_a", OutcomeRange::default())
            .await
            .unwrap();
        assert!(summary.by_kind.contains_key("revenue_usd"));
        assert!(summary.by_kind.contains_key("cost_saved_usd"));
        assert!(summary.by_kind.contains_key("hours_saved"));
        assert!((summary.by_kind["revenue_usd"].sum - 100.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn timeseries_returns_day_buckets() {
        let b = InMemoryOutcomesBackend::new();
        b.record(req("deals_closed", 1.0)).await.unwrap();
        b.record(req("deals_closed", 2.0)).await.unwrap();
        let ts = b
            .timeseries("tenant_a", Some("deals_closed"), OutcomeRange::default())
            .await
            .unwrap();
        assert_eq!(ts.len(), 1); // both fall in today
        assert!((ts[0].sum - 3.0).abs() < f64::EPSILON);
        assert_eq!(ts[0].count, 2);
    }

    #[tokio::test]
    async fn cross_tenant_isolation() {
        let b = InMemoryOutcomesBackend::new();
        b.record(req("revenue_usd", 1000.0)).await.unwrap();
        let mut other = req("revenue_usd", 9999.0);
        other.tenant_id = "tenant_b".into();
        b.record(other).await.unwrap();
        let agg = b
            .aggregate("tenant_a", Some("revenue_usd"), OutcomeRange::default())
            .await
            .unwrap();
        assert!((agg.sum - 1000.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn range_shorthand_filters_records() {
        let b = InMemoryOutcomesBackend::new();
        b.record(req("tickets_resolved", 5.0)).await.unwrap();
        // "24h" should include just-recorded entries.
        let range = OutcomeRange::from_shorthand("24h").unwrap();
        let agg = b
            .aggregate("tenant_a", Some("tickets_resolved"), range)
            .await
            .unwrap();
        assert_eq!(agg.count, 1);
    }

    #[tokio::test]
    async fn writer_rejects_negative_value() {
        let b = InMemoryOutcomesBackend::new();
        let mut bad = req("revenue_usd", -1.0);
        bad.value = -1.0;
        let err = b.record(bad).await.unwrap_err();
        assert!(matches!(err, OutcomesApiError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn writer_rejects_empty_kind() {
        let b = InMemoryOutcomesBackend::new();
        let mut bad = req("", 10.0);
        bad.kind = String::new();
        let err = b.record(bad).await.unwrap_err();
        assert!(matches!(err, OutcomesApiError::InvalidArgument(_)));
    }

    #[test]
    fn record_outcome_request_serialises() {
        let r = req("revenue_usd", 123.45);
        let s = serde_json::to_string(&r).unwrap();
        let back: RecordOutcomeRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.kind, "revenue_usd");
        assert!((back.value - 123.45).abs() < 0.001);
    }
}
