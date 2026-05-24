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
//!   cost_usd = (SUM(prompt_tokens) * cost_per_1k_input_usd
//!             + SUM(completion_tokens) * cost_per_1k_output_usd) / 1000
//!
//! `cost_cents` is `None` for a bucket when ANY token_usage row in that
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
use sqlx::{PgPool, Row};
use xiaoguai_api::usage::{
    UsageError, UsageGroupBy, UsageQuery, UsageReader, UsageReport, UsageRow,
};
use xiaoguai_storage::repositories::begin_tenant_tx;

pub struct PgUsageReader {
    pool: PgPool,
}

impl PgUsageReader {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn arc(pool: PgPool) -> Arc<dyn UsageReader> {
        Arc::new(Self::new(pool))
    }
}

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
        let bucket_sql = bucket_expr(query.group_by);
        let sql = format!(
            "SELECT {bucket_sql} AS bucket, \
                    COALESCE(SUM(tu.prompt_tokens), 0)::BIGINT AS in_tokens, \
                    COALESCE(SUM(tu.completion_tokens), 0)::BIGINT AS out_tokens, \
                    CASE \
                        WHEN COUNT(*) FILTER \
                             (WHERE lp.cost_per_1k_input_usd IS NULL \
                               OR   lp.cost_per_1k_output_usd IS NULL) = 0 \
                        THEN ( \
                            SUM(tu.prompt_tokens::FLOAT8 * lp.cost_per_1k_input_usd) \
                          + SUM(tu.completion_tokens::FLOAT8 * lp.cost_per_1k_output_usd) \
                        ) / 1000.0 \
                        ELSE NULL \
                    END::FLOAT8 AS cost_usd \
             FROM token_usage tu \
             LEFT JOIN llm_providers lp \
                    ON lp.id = tu.provider_id \
                   AND lp.tenant_id IS NULL \
             WHERE ($1::TEXT        IS NULL OR tu.tenant_id = $1) \
               AND ($2::TIMESTAMPTZ IS NULL OR tu.ts >= $2) \
               AND ($3::TIMESTAMPTZ IS NULL OR tu.ts <= $3) \
             GROUP BY bucket \
             ORDER BY bucket ASC"
        );

        let rows = if let Some(t) = &query.tenant_id {
            let mut tx = begin_tenant_tx(&self.pool, Some(t))
                .await
                .map_err(|e| UsageError::Backend(format!("begin tenant tx: {e}")))?;
            let rows = sqlx::query(&sql)
                .bind(Some(t.clone()))
                .bind(query.since)
                .bind(query.until)
                .fetch_all(&mut *tx)
                .await
                .map_err(map_err)?;
            tx.commit().await.map_err(map_err)?;
            rows
        } else {
            sqlx::query(&sql)
                .bind::<Option<String>>(None)
                .bind(query.since)
                .bind(query.until)
                .fetch_all(&self.pool)
                .await
                .map_err(map_err)?
        };

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

            // v1.1.1.1: cost_usd is a NUMERIC computed by the SQL; map to
            // cost_cents (u64) by multiplying by 100 and truncating. NULL
            // means at least one row in this bucket had no provider rates.
            let cost_cents: Option<u64> = r
                .try_get::<Option<f64>, _>("cost_usd")
                .ok()
                .flatten()
                .map(|usd| {
                    let cents = usd * 100.0;
                    // Guard against NaN / negative / overflow from bad data.
                    if cents.is_finite() && cents >= 0.0 {
                        cents.round() as u64
                    } else {
                        0
                    }
                });

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

        Ok(UsageReport {
            rows: out_rows,
            total_input_tokens: total_in,
            total_output_tokens: total_out,
            cost_cents: report_cost,
        })
    }
}

fn bucket_expr(group_by: UsageGroupBy) -> &'static str {
    match group_by {
        UsageGroupBy::Day => "to_char(tu.ts AT TIME ZONE 'UTC', 'YYYY-MM-DD')",
        UsageGroupBy::Provider => "tu.provider_id",
        UsageGroupBy::Model => "tu.model",
    }
}

#[cfg(test)]
mod tests {
    //! Query-string smoke tests. The full PG end-to-end test would need
    //! a testcontainer (`#[ignore]` per project conventions); these
    //! confirm the `group_by` → SQL fragment mapping the bridge feeds
    //! into the query builder.

    use super::*;

    #[test]
    fn bucket_expr_per_group_by() {
        assert!(bucket_expr(UsageGroupBy::Day).contains("to_char"));
        assert_eq!(bucket_expr(UsageGroupBy::Provider), "tu.provider_id");
        assert_eq!(bucket_expr(UsageGroupBy::Model), "tu.model");
    }

    #[tokio::test]
    #[ignore = "requires PG testcontainer; covered manually via xiaoguai-storage smoke"]
    async fn aggregate_round_trip() {
        // Placeholder for a future testcontainer-backed test. The
        // StaticUsageReader unit tests already cover the aggregation
        // semantics; this would assert the SQL parses + groups
        // identically against a real Postgres.
    }
}
