//! `xiaoguai stats` — token-usage + cost observability over the local `SQLite`
//! store (DEC-033 single-user pivot, Phase 4b).
//!
//! Reads the `token_usage` ledger joined to `llm_providers` cost rates and
//! prints a grouped summary (by model / day / session) plus a TOTAL row.
//! Cost = `prompt/1000*input_rate` + `completion/1000*output_rate`. When a
//! provider has no configured rate the cost is shown as `—` (not `0`).
//!
//! All queries are `SQLite`-dialect with `?` placeholders; there is no
//! `tenant_id` in the single-user schema.

use anyhow::{bail, Context, Result};
use sqlx::{Row, SqlitePool};

/// Group-by dimension for the summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    Model,
    Day,
    Session,
}

impl GroupBy {
    /// Parse the `--by` flag value.
    ///
    /// # Errors
    /// Returns an error for any value other than `model`, `day`, or `session`.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "model" => Ok(Self::Model),
            "day" => Ok(Self::Day),
            "session" => Ok(Self::Session),
            other => bail!("unknown --by value '{other}': expected one of model, day, session"),
        }
    }

    /// Column header for the group key.
    #[must_use]
    pub const fn header(self) -> &'static str {
        match self {
            Self::Model => "MODEL",
            Self::Day => "DAY",
            Self::Session => "SESSION",
        }
    }

    /// SQL expression that produces the group key.
    const fn key_expr(self) -> &'static str {
        match self {
            Self::Model => "u.model",
            Self::Day => "substr(u.ts, 1, 10)",
            Self::Session => "u.session_id",
        }
    }
}

/// One aggregated row of the usage summary.
#[derive(Debug, Clone, PartialEq)]
pub struct StatsRow {
    pub key: String,
    pub calls: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    /// `None` when at least one contributing row had no configured cost rate
    /// (so the estimate would be misleading), otherwise the summed estimate.
    pub est_cost_usd: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct StatsArgs {
    pub by: GroupBy,
    pub since: Option<String>,
    pub until: Option<String>,
}

/// Query the `token_usage` ledger and return the grouped summary rows.
///
/// Rows are ordered by `total_tokens` descending. `est_cost_usd` is `None` for
/// a group if any contributing usage row's provider has a NULL cost rate.
///
/// # Errors
/// Returns an error if the SQL query fails.
pub async fn query(pool: &SqlitePool, args: &StatsArgs) -> Result<Vec<StatsRow>> {
    let key = args.by.key_expr();
    // `n_priced` counts rows whose provider has both cost rates set; when it is
    // below `n_rows`, at least one row was unpriced, so the group's cost is
    // surfaced as unknown (`—`) rather than a misleadingly low number.
    let mut sql = format!(
        "SELECT \
            COALESCE({key}, '(none)') AS grp, \
            COUNT(*) AS calls, \
            COALESCE(SUM(u.prompt_tokens), 0) AS prompt_tokens, \
            COALESCE(SUM(u.completion_tokens), 0) AS completion_tokens, \
            COALESCE(SUM(u.total_tokens), 0) AS total_tokens, \
            COUNT(*) AS n_rows, \
            SUM(CASE WHEN p.cost_per_1k_input_usd IS NOT NULL \
                      AND p.cost_per_1k_output_usd IS NOT NULL THEN 1 ELSE 0 END) AS n_priced, \
            SUM(COALESCE(u.prompt_tokens, 0) / 1000.0 * p.cost_per_1k_input_usd \
              + COALESCE(u.completion_tokens, 0) / 1000.0 * p.cost_per_1k_output_usd) AS cost \
         FROM token_usage u \
         LEFT JOIN llm_providers p ON u.provider_id = p.id"
    );

    let mut clauses: Vec<&str> = Vec::new();
    if args.since.is_some() {
        clauses.push("u.ts >= ?");
    }
    if args.until.is_some() {
        clauses.push("u.ts <= ?");
    }
    if !clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
    }
    sql.push_str(" GROUP BY grp ORDER BY total_tokens DESC");

    let mut q = sqlx::query(&sql);
    if let Some(s) = &args.since {
        q = q.bind(s);
    }
    if let Some(u) = &args.until {
        q = q.bind(u);
    }

    let rows = q.fetch_all(pool).await.context("query token_usage")?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let n_rows: i64 = r.try_get("n_rows")?;
        let n_priced: i64 = r.try_get("n_priced")?;
        // Cost is only trustworthy if every contributing row was priced.
        let est_cost_usd = if n_priced == n_rows {
            Some(r.try_get::<f64, _>("cost").unwrap_or(0.0))
        } else {
            None
        };
        out.push(StatsRow {
            key: r.try_get("grp")?,
            calls: r.try_get("calls")?,
            prompt_tokens: r.try_get("prompt_tokens")?,
            completion_tokens: r.try_get("completion_tokens")?,
            total_tokens: r.try_get("total_tokens")?,
            est_cost_usd,
        });
    }
    Ok(out)
}

/// Render the summary rows as a fixed-width text table with a TOTAL row.
#[must_use]
pub fn format_table(rows: &[StatsRow], by: GroupBy) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<28} {:>7} {:>13} {:>13} {:>13} {:>13}",
        by.header(),
        "CALLS",
        "PROMPT_TOK",
        "COMPL_TOK",
        "TOTAL_TOK",
        "EST_COST_USD"
    );
    let mut t_calls = 0_i64;
    let mut t_prompt = 0_i64;
    let mut t_compl = 0_i64;
    let mut t_total = 0_i64;
    let mut t_cost = 0.0_f64;
    let mut cost_complete = true;
    for r in rows {
        let cost = r
            .est_cost_usd
            .map_or_else(|| "—".to_string(), |c| format!("{c:.4}"));
        let _ = writeln!(
            out,
            "{:<28} {:>7} {:>13} {:>13} {:>13} {:>13}",
            truncate(&r.key, 28),
            r.calls,
            r.prompt_tokens,
            r.completion_tokens,
            r.total_tokens,
            cost
        );
        t_calls += r.calls;
        t_prompt += r.prompt_tokens;
        t_compl += r.completion_tokens;
        t_total += r.total_tokens;
        match r.est_cost_usd {
            Some(c) => t_cost += c,
            None => cost_complete = false,
        }
    }
    let total_cost = if cost_complete {
        format!("{t_cost:.4}")
    } else {
        "—".to_string()
    };
    let _ = writeln!(
        out,
        "{:<28} {:>7} {:>13} {:>13} {:>13} {:>13}",
        "TOTAL", t_calls, t_prompt, t_compl, t_total, total_cost
    );
    out
}

/// Render the summary rows as JSON (array of objects + a `total` object).
#[must_use]
pub fn to_json(rows: &[StatsRow]) -> serde_json::Value {
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "key": r.key,
                "calls": r.calls,
                "prompt_tokens": r.prompt_tokens,
                "completion_tokens": r.completion_tokens,
                "total_tokens": r.total_tokens,
                "est_cost_usd": r.est_cost_usd,
            })
        })
        .collect();
    let cost_complete = rows.iter().all(|r| r.est_cost_usd.is_some());
    let total_cost: Option<f64> = if cost_complete {
        Some(rows.iter().filter_map(|r| r.est_cost_usd).sum())
    } else {
        None
    };
    serde_json::json!({
        "rows": items,
        "total": {
            "calls": rows.iter().map(|r| r.calls).sum::<i64>(),
            "prompt_tokens": rows.iter().map(|r| r.prompt_tokens).sum::<i64>(),
            "completion_tokens": rows.iter().map(|r| r.completion_tokens).sum::<i64>(),
            "total_tokens": rows.iter().map(|r| r.total_tokens).sum::<i64>(),
            "est_cost_usd": total_cost,
        }
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}
