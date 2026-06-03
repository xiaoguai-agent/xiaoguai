//! v1.1.1 / v1.1.1.1 — PG-backed adapter for `xiaoguai_api::usage::UsageReader`.
//!
//! Same layering choice as `today_bridge.rs`: the api crate doesn't
//! depend on sqlx, so the aggregation SQL lives here and the api side
//! stays storage-agnostic.
//!
//! The aggregation is a single `GROUP BY` over `token_usage` LEFT-JOINed
//! with `llm_providers` (migration 0003 / 0010) with optional tenant /
//! since / until filters. The bucket column depends on `group_by`:
//!
//!   * `Day`      → `to_char(ts AT TIME ZONE 'UTC', 'YYYY-MM-DD')`
//!   * `Provider` → `provider_id`
//!   * `Model`    → `model`
//!
//! v1.1.1.1 cost computation (migration 0010):
//! ```text
//!   cost_usd = (SUM(prompt_tokens) * cost_per_1k_input_usd
//!             + SUM(completion_tokens) * cost_per_1k_output_usd) / 1000
//! ```
//!
//! `cost_cents` is `None` for a bucket when ANY `token_usage` row in that
//! bucket lacks a matching provider with rates (NULL rates → operator
//! hasn't configured pricing). This matches the partial-cost semantics
//! in `StaticUsageReader`.
//!
//! RLS note: when `tenant_id` is provided we run inside a
//! `begin_tenant_tx` so the `tenant_isolation_token_usage` policy
//! filters to the right rows; when `tenant_id` is `None` we run as the
//! superuser pool which bypasses RLS (admin cross-tenant view, same
//! pattern as `PgTodayReader`).

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::Row;
use xiaoguai_api::usage::{
    UsageError, UsageGroupBy, UsageQuery, UsageReader, UsageReport, UsageRow,
};
use xiaoguai_storage::{repositories::begin_tenant_tx, ReadWritePool};

pub struct PgUsageReader {
    /// Read/write pool: cross-tenant admin reads routed to replica;
    /// tenant-scoped reads run inside a transaction on the primary.
    pool: ReadWritePool,
}

impl PgUsageReader {
    #[must_use]
    pub fn new(pool: ReadWritePool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn arc(pool: ReadWritePool) -> Arc<dyn UsageReader> {
        Arc::new(Self::new(pool))
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "used as `.map_err(map_err)` — changing to `&e` would require closure wrappers at every call site"
)]
fn map_err(e: sqlx::Error) -> UsageError {
    UsageError::Backend(e.to_string())
}

#[async_trait]
impl UsageReader for PgUsageReader {
    async fn aggregate(&self, query: UsageQuery) -> Result<UsageReport, UsageError> {
        if let (Some(since), Some(until)) = (query.since, query.until) {
            if since > until {
                return Err(UsageError::InvalidArgument("since must be <= until".into()));
            }
        }

        // Build a parameterised query with stable positional binds. We
        // always bind in the same order — tenant_id ($1), since ($2),
        // until ($3) — and switch to NULL when a filter is absent so the
        // SQL string itself is constant per group_by.
        //
        // v1.1.1.1: LEFT JOIN llm_providers to pick up cost rates. The
        // join is on token_usage.provider_id = llm_providers.id, which
        // only matches system-wide provider rows (tenant_id IS NULL in
        // llm_providers). This is intentional: cost rates are a system-level
        // configuration, not per-tenant.
        //
        // cost_usd per bucket is only non-NULL when EVERY row in that
        // bucket has a matching provider with both rates set. We achieve
        // this with:
        //   CASE WHEN COUNT(*) FILTER (WHERE lp.cost_per_1k_input_usd IS NULL) = 0
        //        THEN (SUM(tu.prompt_tokens) * MAX(lp.cost_per_1k_input_usd)
        //             + SUM(tu.completion_tokens) * MAX(lp.cost_per_1k_output_usd)) / 1000
        //        ELSE NULL
        //   END
        // Using MAX(lp.cost_*) is valid within a single-provider bucket
        // (Provider group_by) and is a reasonable aggregation for Day/Model
        // buckets when a single provider dominates. Operators can always
        // switch to group_by=provider to get precise per-provider costs.
        // DEC-033: token_usage / llm_providers lost their tenant_id columns
        // (single implicit owner). The vestigial `tenant_id` filter is
        // dropped; since/until are each referenced twice → numbered binds.
        let bucket_sql = bucket_expr(query.group_by);
        let sql = format!(
            "SELECT {bucket_sql} AS bucket, \
                    COALESCE(SUM(tu.prompt_tokens), 0) AS in_tokens, \
                    COALESCE(SUM(tu.completion_tokens), 0) AS out_tokens, \
                    CASE \
                        WHEN COUNT(*) FILTER \
                             (WHERE lp.cost_per_1k_input_usd IS NULL \
                               OR   lp.cost_per_1k_output_usd IS NULL) = 0 \
                        THEN ( \
                            SUM(CAST(tu.prompt_tokens AS REAL) * lp.cost_per_1k_input_usd) \
                          + SUM(CAST(tu.completion_tokens AS REAL) * lp.cost_per_1k_output_usd) \
                        ) / 1000.0 \
                        ELSE NULL \
                    END AS cost_usd \
             FROM token_usage tu \
             LEFT JOIN llm_providers lp \
                    ON lp.id = tu.provider_id \
             WHERE (?1 IS NULL OR tu.ts >= ?1) \
               AND (?2 IS NULL OR tu.ts <= ?2) \
             GROUP BY bucket \
             ORDER BY bucket ASC"
        );

        let rows = if let Some(t) = &query.tenant_id {
            // Tenant scoping is vestigial under DEC-033; `begin_tenant_tx`
            // opens a plain SQLite transaction and ignores the tenant.
            let mut tx = begin_tenant_tx(self.pool.writer(), Some(t))
                .await
                .map_err(|e| UsageError::Backend(format!("begin tx: {e}")))?;
            let rows = sqlx::query(&sql)
                .bind(query.since)
                .bind(query.until)
                .fetch_all(&mut *tx)
                .await
                .map_err(map_err)?;
            tx.commit().await.map_err(map_err)?;
            rows
        } else {
            sqlx::query(&sql)
                .bind(query.since)
                .bind(query.until)
                .fetch_all(self.pool.reader())
                .await
                .map_err(map_err)?
        };

        Ok(build_report(rows))
    }
}

/// Map a set of raw SQL rows into a [`UsageReport`].
///
/// Accumulates token totals and cost cents, propagating `None` cost to the
/// report level when any bucket lacked provider pricing data.
fn build_report(rows: Vec<sqlx::sqlite::SqliteRow>) -> UsageReport {
    let mut total_in: u64 = 0;
    let mut total_out: u64 = 0;
    let mut total_cost_cents: u64 = 0;
    let mut any_missing_cost = false;
    let mut any_row = false;
    let mut out_rows: Vec<UsageRow> = Vec::with_capacity(rows.len());

    for r in rows {
        any_row = true;
        let bucket: String = r
            .try_get::<String, _>("bucket")
            .unwrap_or_else(|_| String::new());
        let in_tokens: i64 = r.try_get::<i64, _>("in_tokens").unwrap_or(0);
        let out_tokens: i64 = r.try_get::<i64, _>("out_tokens").unwrap_or(0);
        let input = u64::try_from(in_tokens.max(0)).unwrap_or(0);
        let output = u64::try_from(out_tokens.max(0)).unwrap_or(0);
        total_in = total_in.saturating_add(input);
        total_out = total_out.saturating_add(output);

        // v1.1.1.1: cost_usd from the SQL CASE expression; NULL means at
        // least one row in this bucket had no provider rates configured.
        let cost_cents: Option<u64> = r
            .try_get::<Option<f64>, _>("cost_usd")
            .ok()
            .flatten()
            .map(cents_from_usd);

        if cost_cents.is_none() {
            any_missing_cost = true;
        } else if let Some(c) = cost_cents {
            total_cost_cents = total_cost_cents.saturating_add(c);
        }

        out_rows.push(UsageRow {
            bucket,
            input_tokens: input,
            output_tokens: output,
            cost_cents,
        });
    }

    // Report-level cost is the sum only when every bucket had a cost.
    // When no rows at all we also return None (consistent with
    // StaticUsageReader's behaviour on an empty dataset).
    let report_cost = if !any_row || any_missing_cost {
        None
    } else {
        Some(total_cost_cents)
    };

    UsageReport {
        rows: out_rows,
        total_input_tokens: total_in,
        total_output_tokens: total_out,
        cost_cents: report_cost,
    }
}

/// Convert a USD float (from the SQL `cost_usd` column) to integer cents.
///
/// Returns 0 for NaN, infinite, or negative values — those indicate bad data
/// in `llm_providers` rates. The caller already guards against NULL (i.e.
/// the `cost_usd IS NULL` path is handled before this is called).
fn cents_from_usd(usd: f64) -> u64 {
    let cents = usd * 100.0;
    // Safety: we check `is_finite() && >= 0.0` before casting, so the value
    // is a non-negative finite f64 in [0, f64::MAX). Truncation is intentional
    // (sub-cent amounts are negligible for display purposes).
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    if cents.is_finite() && cents >= 0.0 {
        cents.round() as u64
    } else {
        0
    }
}

fn bucket_expr(group_by: UsageGroupBy) -> &'static str {
    match group_by {
        // DEC-033: `tu.ts` is stored as the SQLite strftime('%Y-%m-%dT%H:%M:%SZ', 'now') text
        // format ("YYYY-MM-DD HH:MM:SS"); substr(.,1,10) is the day bucket.
        UsageGroupBy::Day => "substr(tu.ts, 1, 10)",
        UsageGroupBy::Provider => "tu.provider_id",
        UsageGroupBy::Model => "tu.model",
    }
}

#[cfg(test)]
mod tests {
    //! Query-string smoke tests + a `SQLite` round-trip. These confirm the
    //! `group_by` → SQL fragment mapping the bridge feeds into the query
    //! builder, and that the aggregation actually parses + groups against
    //! a real (temp) `SQLite` database under `DEC-033`.

    use super::*;

    #[test]
    fn bucket_expr_per_group_by() {
        // DEC-033: the day bucket now uses SQLite `substr`, not PG `to_char`.
        assert!(bucket_expr(UsageGroupBy::Day).contains("substr"));
        assert_eq!(bucket_expr(UsageGroupBy::Provider), "tu.provider_id");
        assert_eq!(bucket_expr(UsageGroupBy::Model), "tu.model");
    }

    #[tokio::test]
    async fn aggregate_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let pool = xiaoguai_storage::db::connect(
            dir.path().join("t.db").to_str().unwrap(),
            5,
        )
        .await
        .unwrap();
        xiaoguai_storage::db::migrate(&pool).await.unwrap();

        // Seed one provider with rates and two token_usage rows on the
        // same day so the Day bucket aggregates both.
        sqlx::query(
            "INSERT INTO llm_providers \
                 (id, name, kind, endpoint, cost_per_1k_input_usd, cost_per_1k_output_usd) \
             VALUES ('prov-a', 'Provider A', 'openai_compat', 'http://x', 1.0, 2.0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO token_usage (provider_id, model, prompt_tokens, completion_tokens, ts) \
             VALUES ('prov-a', 'm1', 1000, 500, '2026-01-01T10:00:00Z'), \
                    ('prov-a', 'm1', 2000, 1000, '2026-01-01T12:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let reader = PgUsageReader::new(pool.into());
        let report = reader
            .aggregate(UsageQuery {
                tenant_id: None,
                since: None,
                until: None,
                group_by: UsageGroupBy::Day,
            })
            .await
            .expect("aggregate");

        assert_eq!(report.rows.len(), 1, "both rows fall in one day bucket");
        assert_eq!(report.rows[0].bucket, "2026-01-01");
        assert_eq!(report.total_input_tokens, 3000);
        assert_eq!(report.total_output_tokens, 1500);
        // cost = (3000*1.0 + 1500*2.0) / 1000 = 6.0 USD = 600 cents
        assert_eq!(report.cost_cents, Some(600));
    }
}
