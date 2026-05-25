//! `xiaoguai watch ...` — manage declarative active-wakeup watchers via the
//! REST API.
//!
//! Talks to `GET /v1/watch`, `POST /v1/watch`, `DELETE /v1/watch/:id`, and
//! `POST /v1/watch/:id/test`.  On HTTP 503 prints a friendly message
//! explaining that the Pg bridge ships in v1.3.

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde_json::Value as JsonValue;

const ERR_503: &str = "Endpoint returns 503 — Pg bridge ships in v1.3. Check /healthz.";

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
// list
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ListArgs {
    pub api_base: String,
    pub tenant_id: Option<String>,
}

pub async fn list(args: ListArgs) -> Result<Vec<JsonValue>> {
    let client = Client::new();
    let mut url = format!("{}/v1/watch", args.api_base);
    if let Some(tid) = &args.tenant_id {
        url.push_str(&format!("?tenant_id={tid}"));
    }
    let resp = client.get(&url).send().await.context("GET /v1/watch")?;
    let resp = require_ok(resp).await?;
    let v: Vec<JsonValue> = resp.json().await.context("decode watch list body")?;
    Ok(v)
}

// ---------------------------------------------------------------------------
// start
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct StartArgs {
    pub api_base: String,
    pub file: std::path::PathBuf,
    pub tenant_id: Option<String>,
}

pub async fn start(args: StartArgs) -> Result<JsonValue> {
    let raw = std::fs::read_to_string(&args.file)
        .with_context(|| format!("read watch spec file: {}", args.file.display()))?;
    let mut spec: JsonValue = serde_yaml::from_str(&raw).context("parse watch spec YAML")?;
    if let Some(tid) = &args.tenant_id {
        if let Some(obj) = spec.as_object_mut() {
            obj.insert("tenant_id".to_string(), JsonValue::String(tid.clone()));
        }
    }
    let client = Client::new();
    let resp = client
        .post(format!("{}/v1/watch", args.api_base))
        .json(&spec)
        .send()
        .await
        .context("POST /v1/watch")?;
    let resp = require_ok(resp).await?;
    let v: JsonValue = resp.json().await.context("decode watch start body")?;
    Ok(v)
}

// ---------------------------------------------------------------------------
// stop
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct StopArgs {
    pub api_base: String,
    pub id: String,
}

pub async fn stop(args: StopArgs) -> Result<()> {
    let client = Client::new();
    let resp = client
        .delete(format!("{}/v1/watch/{}", args.api_base, args.id))
        .send()
        .await
        .context("DELETE /v1/watch/:id")?;
    require_ok(resp).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// test
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TestArgs {
    pub api_base: String,
    pub id: String,
}

pub async fn test(args: TestArgs) -> Result<JsonValue> {
    let client = Client::new();
    let resp = client
        .post(format!("{}/v1/watch/{}/test", args.api_base, args.id))
        .json(&serde_json::json!({}))
        .send()
        .await
        .context("POST /v1/watch/:id/test")?;
    let resp = require_ok(resp).await?;
    let v: JsonValue = resp.json().await.context("decode watch test body")?;
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
        "{:<20} {:<18} {:<8} {:<10} STATUS",
        "ID", "SCHEDULE", "SOURCE", "ACTION"
    );
    for r in rows {
        let id = r.get("id").and_then(JsonValue::as_str).unwrap_or("-");
        let schedule = r.get("schedule").and_then(JsonValue::as_str).unwrap_or("-");
        let source = r.get("source").and_then(JsonValue::as_str).unwrap_or("-");
        let action = r.get("action").and_then(JsonValue::as_str).unwrap_or("-");
        let status = r.get("status").and_then(JsonValue::as_str).unwrap_or("-");
        let _ = writeln!(
            out,
            "{id:<20} {schedule:<18} {source:<8} {action:<10} {status}"
        );
    }
    out
}
