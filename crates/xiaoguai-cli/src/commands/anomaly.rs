//! `xiaoguai anomaly ...` — manage time-series anomaly monitors via the REST
//! API.
//!
//! Talks to `POST /v1/anomaly/run` and `POST /v1/anomaly/test`.
//! On HTTP 503 prints a friendly message explaining that the Pg bridge ships
//! in v1.3.

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
// run
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RunArgs {
    pub api_base: String,
    pub file: std::path::PathBuf,
}

pub async fn run(args: RunArgs) -> Result<JsonValue> {
    let raw = std::fs::read_to_string(&args.file)
        .with_context(|| format!("read anomaly spec file: {}", args.file.display()))?;
    let spec: JsonValue =
        serde_yaml::from_str(&raw).context("parse anomaly spec YAML")?;
    let client = Client::new();
    let resp = client
        .post(format!("{}/v1/anomaly/run", args.api_base))
        .json(&spec)
        .send()
        .await
        .context("POST /v1/anomaly/run")?;
    let resp = require_ok(resp).await?;
    let v: JsonValue = resp.json().await.context("decode anomaly run body")?;
    Ok(v)
}

// ---------------------------------------------------------------------------
// test (back-test)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BacktestArgs {
    pub api_base: String,
    pub file: std::path::PathBuf,
    pub data: std::path::PathBuf,
    pub ts_col: String,
    pub val_col: String,
}

pub async fn backtest(args: BacktestArgs) -> Result<JsonValue> {
    let raw_spec = std::fs::read_to_string(&args.file)
        .with_context(|| format!("read anomaly spec file: {}", args.file.display()))?;
    let spec: JsonValue =
        serde_yaml::from_str(&raw_spec).context("parse anomaly spec YAML")?;
    let csv_content = std::fs::read_to_string(&args.data)
        .with_context(|| format!("read data CSV: {}", args.data.display()))?;
    let client = Client::new();
    let body = serde_json::json!({
        "spec": spec,
        "csv": csv_content,
        "ts_col": args.ts_col,
        "val_col": args.val_col,
    });
    let resp = client
        .post(format!("{}/v1/anomaly/test", args.api_base))
        .json(&body)
        .send()
        .await
        .context("POST /v1/anomaly/test")?;
    let resp = require_ok(resp).await?;
    let v: JsonValue = resp.json().await.context("decode anomaly test body")?;
    Ok(v)
}

// ---------------------------------------------------------------------------
// Table formatting
// ---------------------------------------------------------------------------

#[must_use]
pub fn format_backtest_table(result: &JsonValue) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<8} {:<22} {:<8} {:<8} {:<8} {:<8} {}",
        "ANOMALY", "TS", "VALUE", "MEAN", "STD", "SCORE", "DESCRIPTION"
    );
    if let Some(rows) = result.get("anomalies").and_then(JsonValue::as_array) {
        for r in rows {
            let ts = r.get("ts").and_then(JsonValue::as_str).unwrap_or("-");
            let value = r
                .get("value")
                .and_then(JsonValue::as_f64)
                .map_or_else(|| "-".to_string(), |f| format!("{f:.1}"));
            let mean = r
                .get("mean")
                .and_then(JsonValue::as_f64)
                .map_or_else(|| "-".to_string(), |f| format!("{f:.1}"));
            let std = r
                .get("std")
                .and_then(JsonValue::as_f64)
                .map_or_else(|| "-".to_string(), |f| format!("{f:.1}"));
            let score = r
                .get("score")
                .and_then(JsonValue::as_f64)
                .map_or_else(|| "-".to_string(), |f| format!("{f:.1}"));
            let desc = r
                .get("description")
                .and_then(JsonValue::as_str)
                .unwrap_or("-");
            let _ = writeln!(
                out,
                "{:<8} {ts:<22} {value:<8} {mean:<8} {std:<8} {score:<8} {desc}",
                "*"
            );
        }
    }
    if let Some(summary) = result.get("summary").and_then(JsonValue::as_str) {
        let _ = writeln!(out, "\nsummary: {summary}");
    }
    out
}
