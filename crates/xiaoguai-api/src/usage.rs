//! v1.1.1 — token-usage aggregation surface.
//!
//! Backs `GET /v1/usage`. The endpoint is the admin-facing view of the
//! `token_usage` ledger (migration 0004): one row per `chat_stream`
//! finalised call, aggregated by day / provider / model so the operator
//! can see "what did tenant X spend in the last 30 days" without paging
//! through every individual row.
//!
//! Layering follows the v0.11.1 `TodayReader` shape — the trait lives
//! here (so route handlers stay storage-agnostic and route tests use a
//! `StaticUsageReader`) and the PG implementation ships in
//! `xiaoguai-core/src/usage_bridge.rs`.
//!
//! Cost computation is deferred: `llm_providers` (migration 0003) has no
//! `cost_per_1k_*` columns today. `aggregate` returns `cost_cents = None`
//! on every row + the report total until that schema lands (tracked in
//! the v1.1.1 plan doc, "deferrals" section).

use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum UsageError {
    #[error("usage backend: {0}")]
    Backend(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

/// What to bucket on. Day buckets are ISO-8601 dates (`YYYY-MM-DD`) in
/// UTC; provider buckets are the `llm_providers.id`; model buckets are
/// the model name string the LLM router recorded with the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageGroupBy {
    Day,
    Provider,
    Model,
}

impl UsageGroupBy {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Provider => "provider",
            Self::Model => "model",
        }
    }
}

impl Default for UsageGroupBy {
    fn default() -> Self {
        Self::Day
    }
}

/// Filter knobs forwarded to the backing reader. `since` / `until` are
/// inclusive bounds on `token_usage.ts`. `tenant_id = None` means
/// cross-tenant aggregation (admin view across the whole deployment).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageQuery {
    pub tenant_id: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub group_by: UsageGroupBy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageRow {
    /// Stringified bucket key. `Day` → `YYYY-MM-DD`. `Provider` →
    /// `llm_providers.id`. `Model` → model name. Always non-empty.
    pub bucket: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// `None` until per-provider cost rates are wired (see module docs).
    pub cost_cents: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageReport {
    pub rows: Vec<UsageRow>,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    /// Sum of every row's `cost_cents`, or `None` when any row is
    /// missing a cost (so the operator sees "partial" rather than a
    /// misleading lower-bound).
    pub cost_cents: Option<u64>,
}

#[async_trait]
pub trait UsageReader: Send + Sync {
    async fn aggregate(&self, query: UsageQuery) -> Result<UsageReport, UsageError>;
}

/// In-memory `UsageReader` for route tests. Holds a fixed list of raw
/// (pre-aggregation) entries; `aggregate` does the group-by + sum in
/// Rust so tests can assert against bucket math without touching PG.
#[derive(Debug, Default, Clone)]
pub struct StaticUsageReader {
    pub entries: Vec<StaticUsageEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticUsageEntry {
    pub ts: DateTime<Utc>,
    pub tenant_id: String,
    pub provider_id: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_cents: Option<u64>,
}

impl StaticUsageReader {
    #[must_use]
    pub fn with_entries(entries: Vec<StaticUsageEntry>) -> Self {
        Self { entries }
    }
}

#[async_trait]
impl UsageReader for StaticUsageReader {
    async fn aggregate(&self, query: UsageQuery) -> Result<UsageReport, UsageError> {
        if let (Some(since), Some(until)) = (query.since, query.until) {
            if since > until {
                return Err(UsageError::InvalidArgument("since must be <= until".into()));
            }
        }

        // Slot tuple: (input_tokens, output_tokens, cost_cents).
        // `cost_cents = None` means at least one entry in this bucket
        // had a missing cost — the bucket and the report total then
        // surface as `None` rather than a misleading lower bound.
        let mut buckets: BTreeMap<String, (u64, u64, Option<u64>)> = BTreeMap::new();
        let mut total_in: u64 = 0;
        let mut total_out: u64 = 0;
        let mut any_missing_cost = false;
        let mut total_cost: u64 = 0;
        let mut any_with_cost = false;

        for e in &self.entries {
            if let Some(t) = &query.tenant_id {
                if &e.tenant_id != t {
                    continue;
                }
            }
            if let Some(since) = query.since {
                if e.ts < since {
                    continue;
                }
            }
            if let Some(until) = query.until {
                if e.ts > until {
                    continue;
                }
            }

            let key = match query.group_by {
                UsageGroupBy::Day => e.ts.format("%Y-%m-%d").to_string(),
                UsageGroupBy::Provider => e.provider_id.clone(),
                UsageGroupBy::Model => e.model.clone(),
            };
            let slot = buckets.entry(key).or_insert((0, 0, Some(0)));
            slot.0 = slot.0.saturating_add(e.input_tokens);
            slot.1 = slot.1.saturating_add(e.output_tokens);
            if let Some(c) = e.cost_cents {
                if let Some(curr) = slot.2 {
                    slot.2 = Some(curr.saturating_add(c));
                }
                total_cost = total_cost.saturating_add(c);
                any_with_cost = true;
            } else {
                slot.2 = None;
                any_missing_cost = true;
            }
            total_in = total_in.saturating_add(e.input_tokens);
            total_out = total_out.saturating_add(e.output_tokens);
        }

        let rows: Vec<UsageRow> = buckets
            .into_iter()
            .map(|(bucket, (input, output, cost))| UsageRow {
                bucket,
                input_tokens: input,
                output_tokens: output,
                cost_cents: cost,
            })
            .collect();

        // Report-level cost is the sum only when every contributing
        // entry had a cost. Mixed / all-missing collapses to None so
        // the UI never shows a misleading partial total.
        let report_cost = if any_missing_cost || !any_with_cost {
            None
        } else {
            Some(total_cost)
        };

        Ok(UsageReport {
            rows,
            total_input_tokens: total_in,
            total_output_tokens: total_out,
            cost_cents: report_cost,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn entry(ts: DateTime<Utc>, tenant: &str, provider: &str, model: &str) -> StaticUsageEntry {
        StaticUsageEntry {
            ts,
            tenant_id: tenant.into(),
            provider_id: provider.into(),
            model: model.into(),
            input_tokens: 100,
            output_tokens: 50,
            cost_cents: None,
        }
    }

    #[tokio::test]
    async fn static_reader_groups_by_day() {
        let d1 = Utc.with_ymd_and_hms(2026, 5, 20, 1, 0, 0).unwrap();
        let d1b = Utc.with_ymd_and_hms(2026, 5, 20, 23, 0, 0).unwrap();
        let d2 = Utc.with_ymd_and_hms(2026, 5, 21, 1, 0, 0).unwrap();
        let reader = StaticUsageReader::with_entries(vec![
            entry(d1, "ten", "openai", "gpt-4o"),
            entry(d1b, "ten", "openai", "gpt-4o"),
            entry(d2, "ten", "openai", "gpt-4o"),
        ]);
        let got = reader
            .aggregate(UsageQuery {
                tenant_id: None,
                since: None,
                until: None,
                group_by: UsageGroupBy::Day,
            })
            .await
            .unwrap();
        assert_eq!(got.rows.len(), 2);
        assert_eq!(got.rows[0].bucket, "2026-05-20");
        assert_eq!(got.rows[0].input_tokens, 200);
        assert_eq!(got.rows[0].output_tokens, 100);
        assert_eq!(got.rows[1].bucket, "2026-05-21");
        assert_eq!(got.total_input_tokens, 300);
        assert_eq!(got.total_output_tokens, 150);
        // No entries carry a cost → report cost is None.
        assert!(got.cost_cents.is_none());
    }

    #[tokio::test]
    async fn static_reader_groups_by_provider_and_model() {
        let ts = Utc::now();
        let reader = StaticUsageReader::with_entries(vec![
            entry(ts, "ten", "openai", "gpt-4o"),
            entry(ts, "ten", "openai", "gpt-4o-mini"),
            entry(ts, "ten", "anthropic", "claude-3-5"),
        ]);
        let by_prov = reader
            .aggregate(UsageQuery {
                tenant_id: None,
                since: None,
                until: None,
                group_by: UsageGroupBy::Provider,
            })
            .await
            .unwrap();
        assert_eq!(by_prov.rows.len(), 2);
        assert!(by_prov.rows.iter().any(|r| r.bucket == "openai"));
        assert!(by_prov.rows.iter().any(|r| r.bucket == "anthropic"));

        let by_model = reader
            .aggregate(UsageQuery {
                tenant_id: None,
                since: None,
                until: None,
                group_by: UsageGroupBy::Model,
            })
            .await
            .unwrap();
        assert_eq!(by_model.rows.len(), 3);
    }

    #[tokio::test]
    async fn static_reader_filters_by_tenant_and_since_until() {
        let d1 = Utc.with_ymd_and_hms(2026, 5, 20, 1, 0, 0).unwrap();
        let d2 = Utc.with_ymd_and_hms(2026, 5, 21, 1, 0, 0).unwrap();
        let d3 = Utc.with_ymd_and_hms(2026, 5, 22, 1, 0, 0).unwrap();
        let reader = StaticUsageReader::with_entries(vec![
            entry(d1, "ten_a", "openai", "gpt-4o"),
            entry(d2, "ten_a", "openai", "gpt-4o"),
            entry(d2, "ten_b", "openai", "gpt-4o"),
            entry(d3, "ten_a", "openai", "gpt-4o"),
        ]);
        let got = reader
            .aggregate(UsageQuery {
                tenant_id: Some("ten_a".into()),
                since: Some(d2),
                until: Some(d2),
                group_by: UsageGroupBy::Day,
            })
            .await
            .unwrap();
        assert_eq!(got.rows.len(), 1);
        assert_eq!(got.rows[0].bucket, "2026-05-21");
        assert_eq!(got.total_input_tokens, 100);
    }

    #[tokio::test]
    async fn static_reader_rejects_since_after_until() {
        let reader = StaticUsageReader::default();
        let earlier = Utc.with_ymd_and_hms(2026, 5, 20, 0, 0, 0).unwrap();
        let later = Utc.with_ymd_and_hms(2026, 5, 21, 0, 0, 0).unwrap();
        let err = reader
            .aggregate(UsageQuery {
                tenant_id: None,
                since: Some(later),
                until: Some(earlier),
                group_by: UsageGroupBy::Day,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, UsageError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn static_reader_propagates_partial_cost() {
        let ts = Utc::now();
        let mut with_cost = entry(ts, "ten", "openai", "gpt-4o");
        with_cost.cost_cents = Some(120);
        let without_cost = entry(ts, "ten", "openai", "gpt-4o-mini");
        let reader = StaticUsageReader::with_entries(vec![with_cost, without_cost]);
        let got = reader
            .aggregate(UsageQuery {
                tenant_id: None,
                since: None,
                until: None,
                group_by: UsageGroupBy::Model,
            })
            .await
            .unwrap();
        // Mixed cost availability => report total cost is None.
        assert!(got.cost_cents.is_none());
        // The bucket with the missing-cost entry must also surface as None.
        let mini = got.rows.iter().find(|r| r.bucket == "gpt-4o-mini").unwrap();
        assert!(mini.cost_cents.is_none());
        let full = got.rows.iter().find(|r| r.bucket == "gpt-4o").unwrap();
        assert_eq!(full.cost_cents, Some(120));
    }

    #[test]
    fn group_by_serialises_snake_case() {
        let s = serde_json::to_string(&UsageGroupBy::Day).unwrap();
        assert_eq!(s, "\"day\"");
        let p = serde_json::to_string(&UsageGroupBy::Provider).unwrap();
        assert_eq!(p, "\"provider\"");
        let m = serde_json::to_string(&UsageGroupBy::Model).unwrap();
        assert_eq!(m, "\"model\"");
        let back: UsageGroupBy = serde_json::from_str("\"provider\"").unwrap();
        assert_eq!(back, UsageGroupBy::Provider);
    }
}
