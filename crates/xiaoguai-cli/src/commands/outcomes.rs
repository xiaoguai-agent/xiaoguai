//! `xiaoguai outcomes ...` — manage agent outcome telemetry via the REST API.
//!
//! Talks to `POST /v1/outcomes`, `GET /v1/outcomes`, `GET /v1/outcomes/summary`,
//! and `GET /v1/outcomes/timeseries`.  On HTTP 503 prints a friendly message
//! explaining that the Pg bridge ships in v1.3.

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde_json::Value as JsonValue;

const ERR_503: &str =
    "Endpoint returns 503 — Pg bridge ships in v1.3. Check /healthz.";

async fn require_ok(resp: reqwest::Response) -> Result<reqwest::Response> {
    let status = resp.status();
    if status.as_u16() == 503 {
        bail!("{ERR_503}");
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("API returned {status}: {body}");
    }
    Ok(resp)
}

// ---------------------------------------------------------------------------
// record
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RecordArgs {
    pub api_base: String,
    pub tenant_id: String,
    pub agent_name: String,
    pub kind: String,
    pub value: f64,
    pub session_id: Option<String>,
    pub unit: Option<String>,
    pub description: Option<String>,
}

pub async fn record(args: RecordArgs) -> Result<JsonValue> {
    let valid_kinds = [
        "revenue_usd",
        "cost_saved_usd",
        "hours_saved",
        "deals_closed",
        "tickets_resolved",
        "custom",
    ];
    if !valid_kinds.contains(&args.kind.as_str()) {
        bail!(
            "unknown kind '{}': expected one of {}",
            args.kind,
            valid_kinds.join(", ")
        );
    }
    if args.value < 0.0 {
        bail!("--value must be non-negative");
    }
    let client = Client::new();
    let body = serde_json::json!({
        "tenant_id": args.tenant_id,
        "agent_name": args.agent_name,
        "kind": args.kind,
        "value": args.value,
        "session_id": args.session_id,
        "unit": args.unit,
        "description": args.description,
    });
    let resp = client
        .post(format!("{}/v1/outcomes", args.api_base))
        .json(&body)
        .send()
        .await
        .context("POST /v1/outcomes")?;
    let resp = require_ok(resp).await?;
    let v: JsonValue = resp.json().await.context("decode record body")?;
    Ok(v)
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ListArgs {
    pub api_base: String,
    pub tenant_id: String,
    pub range: String,
    pub kind: Option<String>,
    pub limit: u32,
}

pub async fn list(args: ListArgs) -> Result<Vec<JsonValue>> {
    let client = Client::new();
    let mut url = format!(
        "{}/v1/outcomes?tenant_id={}&range={}&limit={}",
        args.api_base, args.tenant_id, args.range, args.limit
    );
    if let Some(k) = &args.kind {
        url.push_str(&format!("&kind={k}"));
    }
    let resp = client
        .get(&url)
        .send()
        .await
        .context("GET /v1/outcomes")?;
    let resp = require_ok(resp).await?;
    let v: Vec<JsonValue> = resp.json().await.context("decode list body")?;
    Ok(v)
}

// ---------------------------------------------------------------------------
// summary
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SummaryArgs {
    pub api_base: String,
    pub tenant_id: String,
    pub range: String,
}

pub async fn summary(args: SummaryArgs) -> Result<Vec<JsonValue>> {
    let client = Client::new();
    let url = format!(
        "{}/v1/outcomes/summary?tenant_id={}&range={}",
        args.api_base, args.tenant_id, args.range
    );
    let resp = client
        .get(&url)
        .send()
        .await
        .context("GET /v1/outcomes/summary")?;
    let resp = require_ok(resp).await?;
    let v: Vec<JsonValue> = resp.json().await.context("decode summary body")?;
    Ok(v)
}

// ---------------------------------------------------------------------------
// timeseries
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TimeseriesArgs {
    pub api_base: String,
    pub tenant_id: String,
    pub range: String,
    pub kind: Option<String>,
}

pub async fn timeseries(args: TimeseriesArgs) -> Result<JsonValue> {
    let client = Client::new();
    let mut url = format!(
        "{}/v1/outcomes/timeseries?tenant_id={}&range={}",
        args.api_base, args.tenant_id, args.range
    );
    if let Some(k) = &args.kind {
        url.push_str(&format!("&kind={k}"));
    }
    let resp = client
        .get(&url)
        .send()
        .await
        .context("GET /v1/outcomes/timeseries")?;
    let resp = require_ok(resp).await?;
    let v: JsonValue = resp.json().await.context("decode timeseries body")?;
    Ok(v)
}

// ---------------------------------------------------------------------------
// Table formatting
// ---------------------------------------------------------------------------

#[must_use]
pub fn format_list_table(rows: &[JsonValue]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<22} {:<15} {:<18} {:<10} {}",
        "RECORDED_AT", "AGENT", "KIND", "VALUE", "SESSION"
    );
    for r in rows {
        let ts = r
            .get("recorded_at")
            .and_then(JsonValue::as_str)
            .unwrap_or("-");
        let agent = r
            .get("agent_name")
            .and_then(JsonValue::as_str)
            .unwrap_or("-");
        let kind = r.get("kind").and_then(JsonValue::as_str).unwrap_or("-");
        let value = r
            .get("value")
            .and_then(JsonValue::as_f64)
            .map_or_else(|| "-".to_string(), |f| format!("{f:.2}"));
        let session = r
            .get("session_id")
            .and_then(JsonValue::as_str)
            .unwrap_or("-");
        let _ = writeln!(out, "{ts:<22} {agent:<15} {kind:<18} {value:<10} {session}");
    }
    out
}

#[must_use]
pub fn format_summary_table(rows: &[JsonValue]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<20} {:<14} {:<8} {}",
        "KIND", "TOTAL", "COUNT", "AVG"
    );
    for r in rows {
        let kind = r.get("kind").and_then(JsonValue::as_str).unwrap_or("-");
        let total = r
            .get("total")
            .and_then(JsonValue::as_f64)
            .map_or_else(|| "-".to_string(), |f| format!("{f:.2}"));
        let count = r
            .get("count")
            .and_then(JsonValue::as_u64)
            .map_or_else(|| "-".to_string(), |n| n.to_string());
        let avg = r
            .get("avg")
            .and_then(JsonValue::as_f64)
            .map_or_else(|| "-".to_string(), |f| format!("{f:.2}"));
        let _ = writeln!(out, "{kind:<20} {total:<14} {count:<8} {avg}");
    }
    out
}
