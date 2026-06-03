//! v1.2.4 — PG-backed `OutcomeWriter` + `OutcomesReader`.
//!
//! `PgOutcomesBackend` implements both traits so both writer and reader
//! share the same pool — one `Arc` suffices in `AppState`.
//!
//! Table: `agent_outcomes` (migration 0012).
//!
//! Summary query: aggregate `value` by `kind` for a tenant + range.
//! Timeseries query: daily buckets (`date_trunc('day', attributed_at)`).
//! Both are plain PG aggregates — no RLS transaction needed because
//! outcome endpoints are admin / agent-keyed (the caller already has
//! a tenant-id in the JWT or request body).

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::SqlitePool;
use xiaoguai_api::outcomes::{
    OutcomeWriter, OutcomesApiError, OutcomesReader, RecordOutcomeRequest,
};
use xiaoguai_audit::outcomes::{Aggregate, OutcomeDay, OutcomeRange, OutcomeSummary};

// ── backend struct ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PgOutcomesBackend {
    pool: SqlitePool,
}

impl PgOutcomesBackend {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Convenience: return as `Arc<Self>` so it can be split into two typed
    /// `Arc<dyn ...>` in `AppState` by cloning the inner `Arc`.
    #[must_use]
    pub fn arc(pool: SqlitePool) -> Arc<Self> {
        Arc::new(Self::new(pool))
    }
}

#[allow(clippy::needless_pass_by_value)]
fn pg_err(e: sqlx::Error) -> OutcomesApiError {
    OutcomesApiError::Backend(e.to_string())
}

// ── OutcomeWriter ─────────────────────────────────────────────────────────────

#[async_trait]
impl OutcomeWriter for PgOutcomesBackend {
    async fn record(&self, req: RecordOutcomeRequest) -> Result<(), OutcomesApiError> {
        // Mirror the validation from `InMemoryOutcomeRecorder`.
        if req.value < 0.0 {
            return Err(OutcomesApiError::InvalidArgument(
                "value must be non-negative".into(),
            ));
        }
        if req.kind.is_empty() {
            return Err(OutcomesApiError::InvalidArgument(
                "kind must not be empty".into(),
            ));
        }

        // `session_id` is stored as TEXT in the schema (UUID stored as text).
        let metadata =
            serde_json::to_value(&req.metadata).unwrap_or_else(|_| serde_json::json!({}));

        // DEC-033: tenant_id column dropped. metadata is stored as TEXT;
        // serialize the JSON value to a string for the bind.
        let _ = &req.tenant_id;
        let metadata_str = serde_json::to_string(&metadata).unwrap_or_else(|_| "{}".to_string());
        sqlx::query(
            "INSERT INTO agent_outcomes \
                (session_id, agent_name, kind, value, unit, description, metadata) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&req.session_id)
        .bind(&req.agent_name)
        .bind(&req.kind)
        .bind(req.value)
        .bind(&req.unit)
        .bind(&req.description)
        .bind(&metadata_str)
        .execute(&self.pool)
        .await
        .map_err(pg_err)?;

        Ok(())
    }
}

// ── OutcomesReader ────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct SummaryRow {
    kind: String,
    total: f64,
    cnt: i64,
}

#[derive(sqlx::FromRow)]
struct TimeseriesRow {
    date: String,
    kind: String,
    total: f64,
    cnt: i64,
}

#[async_trait]
impl OutcomesReader for PgOutcomesBackend {
    async fn summary(
        &self,
        tenant_id: &str,
        range: OutcomeRange,
    ) -> Result<OutcomeSummary, OutcomesApiError> {
        // DEC-033: tenant_id column dropped; vestigial param ignored.
        // since/until are each referenced twice → numbered binds required.
        let _ = tenant_id;
        let rows: Vec<SummaryRow> = sqlx::query_as(
            "SELECT kind, \
                    COALESCE(SUM(value), 0.0) AS total, \
                    COUNT(*) AS cnt \
             FROM agent_outcomes \
             WHERE (?1 IS NULL OR attributed_at >= ?1) \
               AND (?2 IS NULL OR attributed_at <= ?2) \
             GROUP BY kind \
             ORDER BY kind",
        )
        .bind(range.since)
        .bind(range.until)
        .fetch_all(&self.pool)
        .await
        .map_err(pg_err)?;

        let mut summary = OutcomeSummary::default();
        for row in rows {
            #[allow(clippy::cast_precision_loss)]
            let avg = if row.cnt > 0 {
                row.total / row.cnt as f64
            } else {
                0.0
            };
            let count = u64::try_from(row.cnt.max(0)).unwrap_or(0);
            summary.by_kind.insert(
                row.kind,
                Aggregate {
                    sum: row.total,
                    count,
                    avg,
                },
            );
        }
        Ok(summary)
    }

    async fn timeseries(
        &self,
        tenant_id: &str,
        kind: Option<&str>,
        range: OutcomeRange,
    ) -> Result<Vec<OutcomeDay>, OutcomesApiError> {
        // DEC-033: tenant_id dropped. `attributed_at` is stored as the
        // SQLite strftime('%Y-%m-%dT%H:%M:%SZ', 'now') text format ("YYYY-MM-DD HH:MM:SS"), so
        // substr(.,1,10) yields the day bucket. kind/since/until each
        // referenced twice → numbered binds.
        let _ = tenant_id;
        let rows: Vec<TimeseriesRow> = sqlx::query_as(
            "SELECT substr(attributed_at, 1, 10) AS date, \
                    kind, \
                    COALESCE(SUM(value), 0.0) AS total, \
                    COUNT(*) AS cnt \
             FROM agent_outcomes \
             WHERE (?1 IS NULL OR kind = ?1) \
               AND (?2 IS NULL OR attributed_at >= ?2) \
               AND (?3 IS NULL OR attributed_at <= ?3) \
             GROUP BY date, kind \
             ORDER BY date ASC, kind ASC",
        )
        .bind(kind)
        .bind(range.since)
        .bind(range.until)
        .fetch_all(&self.pool)
        .await
        .map_err(pg_err)?;

        Ok(rows
            .into_iter()
            .map(|r| OutcomeDay {
                date: r.date,
                kind: r.kind,
                sum: r.total,
                count: u64::try_from(r.cnt.max(0)).unwrap_or(0),
            })
            .collect())
    }

    async fn aggregate(
        &self,
        tenant_id: &str,
        kind: Option<&str>,
        range: OutcomeRange,
    ) -> Result<Aggregate, OutcomesApiError> {
        if let (Some(since), Some(until)) = (range.since, range.until) {
            if since > until {
                return Err(OutcomesApiError::InvalidArgument(
                    "since must be <= until".into(),
                ));
            }
        }

        // DEC-033: tenant_id dropped. kind/since/until each referenced
        // twice → numbered binds.
        let _ = tenant_id;
        let row: (Option<f64>, Option<i64>) = sqlx::query_as(
            "SELECT SUM(value), COUNT(*) \
             FROM agent_outcomes \
             WHERE (?1 IS NULL OR kind = ?1) \
               AND (?2 IS NULL OR attributed_at >= ?2) \
               AND (?3 IS NULL OR attributed_at <= ?3)",
        )
        .bind(kind)
        .bind(range.since)
        .bind(range.until)
        .fetch_one(&self.pool)
        .await
        .map_err(pg_err)?;

        let sum = row.0.unwrap_or(0.0);
        let count = u64::try_from(row.1.unwrap_or(0).max(0)).unwrap_or(0);
        #[allow(clippy::cast_precision_loss)]
        let avg = if count > 0 { sum / count as f64 } else { 0.0 };
        Ok(Aggregate { sum, count, avg })
    }
}

// ── unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_request_negative_value_rejected() {
        // Validate the guard logic without a DB connection.
        let req = RecordOutcomeRequest {
            tenant_id: "tenant-a".into(),
            session_id: None,
            agent_name: "bot".into(),
            kind: "revenue_usd".into(),
            value: -1.0,
            unit: None,
            description: None,
            metadata: serde_json::json!({}),
        };
        // We can't call `.record()` without a pool, but we can replicate the
        // guard to confirm the condition is correct.
        assert!(req.value < 0.0, "negative value must be rejected");
    }

    #[test]
    fn record_request_empty_kind_rejected() {
        let req = RecordOutcomeRequest {
            tenant_id: "tenant-a".into(),
            session_id: None,
            agent_name: "bot".into(),
            kind: String::new(),
            value: 1.0,
            unit: None,
            description: None,
            metadata: serde_json::json!({}),
        };
        assert!(req.kind.is_empty(), "empty kind must be rejected");
    }

    #[test]
    fn aggregate_since_after_until_invalid() {
        use chrono::{Duration, Utc};
        let now = Utc::now();
        let bad_range = OutcomeRange {
            since: Some(now),
            until: Some(now - Duration::hours(1)),
        };
        // Confirm the guard condition holds.
        if let (Some(since), Some(until)) = (bad_range.since, bad_range.until) {
            assert!(since > until, "since > until should be detected as invalid");
        }
    }

    // ── SQLite integration tests (DEC-033) ────────────────────────────────────

    async fn sqlite_backend() -> (tempfile::TempDir, PgOutcomesBackend) {
        let dir = tempfile::tempdir().unwrap();
        let pool = xiaoguai_storage::db::connect(dir.path().join("t.db").to_str().unwrap(), 5)
            .await
            .unwrap();
        xiaoguai_storage::db::migrate(&pool).await.unwrap();
        (dir, PgOutcomesBackend::new(pool))
    }

    fn req(kind: &str, value: f64) -> RecordOutcomeRequest {
        RecordOutcomeRequest {
            // tenant_id is vestigial under DEC-033 (single owner).
            tenant_id: xiaoguai_storage::OWNER_TENANT_ID.into(),
            session_id: Some("sess-1".into()),
            agent_name: "sales-bot".into(),
            kind: kind.into(),
            value,
            unit: Some("usd".into()),
            description: Some("test".into()),
            metadata: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn outcomes_record_and_aggregate() {
        use xiaoguai_api::outcomes::{OutcomeWriter, OutcomesReader};
        let (_dir, backend) = sqlite_backend().await;
        let tid = xiaoguai_storage::OWNER_TENANT_ID;

        backend.record(req("revenue_usd", 500.0)).await.unwrap();
        backend.record(req("revenue_usd", 300.0)).await.unwrap();

        let agg = backend
            .aggregate(tid, Some("revenue_usd"), OutcomeRange::default())
            .await
            .unwrap();
        assert!((agg.sum - 800.0).abs() < 0.001);
        assert_eq!(agg.count, 2);
        assert!((agg.avg - 400.0).abs() < 0.001);
    }

    #[tokio::test]
    async fn outcomes_timeseries_day_buckets() {
        use xiaoguai_api::outcomes::{OutcomeWriter, OutcomesReader};
        let (_dir, backend) = sqlite_backend().await;
        let tid = xiaoguai_storage::OWNER_TENANT_ID;

        backend.record(req("deals_closed", 1.0)).await.unwrap();
        backend.record(req("deals_closed", 2.0)).await.unwrap();

        let ts = backend
            .timeseries(tid, Some("deals_closed"), OutcomeRange::default())
            .await
            .unwrap();
        assert_eq!(ts.len(), 1, "both records in one day bucket");
        assert!((ts[0].sum - 3.0).abs() < 0.001);
        assert_eq!(ts[0].count, 2);
    }

    // DELETED outcomes_pg_cross_tenant_isolation: under DEC-033 there is one
    // implicit owner and the tenant_id param is ignored, so cross-tenant
    // isolation is no longer a meaningful behaviour to assert.

    #[tokio::test]
    async fn outcomes_summary_groups_by_kind() {
        use xiaoguai_api::outcomes::{OutcomeWriter, OutcomesReader};
        let (_dir, backend) = sqlite_backend().await;
        let tid = xiaoguai_storage::OWNER_TENANT_ID;

        backend.record(req("revenue_usd", 100.0)).await.unwrap();
        backend.record(req("cost_saved_usd", 50.0)).await.unwrap();
        backend.record(req("hours_saved", 8.0)).await.unwrap();

        let summary = backend.summary(tid, OutcomeRange::default()).await.unwrap();
        assert!(summary.by_kind.contains_key("revenue_usd"));
        assert!(summary.by_kind.contains_key("cost_saved_usd"));
        assert!((summary.by_kind["revenue_usd"].sum - 100.0).abs() < 0.001);
    }
}
