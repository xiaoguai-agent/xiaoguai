//! v1.1.1 — PG-backed adapter for `xiaoguai_api::usage::UsageReader`.
//!
//! Same layering choice as `today_bridge.rs`: the api crate doesn't
//! depend on sqlx, so the aggregation SQL lives here and the api side
//! stays storage-agnostic.
//!
//! The aggregation is a single `GROUP BY` over `token_usage` (migration
//! 0004) with optional tenant / since / until filters. The bucket column
//! depends on `group_by`:
//!
//!   * `Day`      → `to_char(ts AT TIME ZONE 'UTC', 'YYYY-MM-DD')`
//!   * `Provider` → `provider_id`
//!   * `Model`    → `model`
//!
//! Cost rates are deferred (`llm_providers` has no `cost_per_1k_*`
//! columns today; see api crate module docs). Until those land, every
//! row's `cost_cents` is `None` and the report-level `cost_cents` is
//! also `None`.
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
        let bucket_sql = bucket_expr(query.group_by);
        let sql = format!(
            "SELECT {bucket_sql} AS bucket, \
                    COALESCE(SUM(prompt_tokens), 0)::BIGINT AS in_tokens, \
                    COALESCE(SUM(completion_tokens), 0)::BIGINT AS out_tokens \
             FROM token_usage \
             WHERE ($1::TEXT        IS NULL OR tenant_id = $1) \
               AND ($2::TIMESTAMPTZ IS NULL OR ts >= $2) \
               AND ($3::TIMESTAMPTZ IS NULL OR ts <= $3) \
             GROUP BY bucket \
             ORDER BY bucket ASC"
        );

        let rows = if let Some(t) = &query.tenant_id {
            // Tenant-scoped query: runs inside an RLS transaction on the
            // primary (writes to the same Pg session that SET LOCAL app.tenant).
            let mut tx = begin_tenant_tx(self.pool.writer(), Some(t))
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
            // Cross-tenant admin view: pure read, route to replica.
            sqlx::query(&sql)
                .bind::<Option<String>>(None)
                .bind(query.since)
                .bind(query.until)
                .fetch_all(self.pool.reader())
                .await
                .map_err(map_err)?
        };

        let mut total_in: u64 = 0;
        let mut total_out: u64 = 0;
        let mut out_rows: Vec<UsageRow> = Vec::with_capacity(rows.len());
        for r in rows {
            let bucket: String = r
                .try_get::<String, _>("bucket")
                .unwrap_or_else(|_| String::new());
            let in_tokens: i64 = r.try_get::<i64, _>("in_tokens").unwrap_or(0);
            let out_tokens: i64 = r.try_get::<i64, _>("out_tokens").unwrap_or(0);
            let input = u64::try_from(in_tokens.max(0)).unwrap_or(0);
            let output = u64::try_from(out_tokens.max(0)).unwrap_or(0);
            total_in = total_in.saturating_add(input);
            total_out = total_out.saturating_add(output);
            out_rows.push(UsageRow {
                bucket,
                input_tokens: input,
                output_tokens: output,
                // Cost rates deferred (see module docs).
                cost_cents: None,
            });
        }

        Ok(UsageReport {
            rows: out_rows,
            total_input_tokens: total_in,
            total_output_tokens: total_out,
            cost_cents: None,
        })
    }
}

fn bucket_expr(group_by: UsageGroupBy) -> &'static str {
    match group_by {
        UsageGroupBy::Day => "to_char(ts AT TIME ZONE 'UTC', 'YYYY-MM-DD')",
        UsageGroupBy::Provider => "provider_id",
        UsageGroupBy::Model => "model",
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
        assert_eq!(bucket_expr(UsageGroupBy::Provider), "provider_id");
        assert_eq!(bucket_expr(UsageGroupBy::Model), "model");
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
