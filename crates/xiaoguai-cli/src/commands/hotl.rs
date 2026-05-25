//! `xiaoguai hotl ...` — administer Human-on-the-Loop budget policies via the
//! REST API.
//!
//! All operations go through HTTP; no direct DB access. On HTTP 503 the CLI
//! prints a friendly message explaining that the Pg bridge ships in v1.3.

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value as JsonValue;

const ERR_503: &str = "Endpoint returns 503 — Pg bridge ships in v1.3. Check /healthz.";

/// Shared HTTP helper — checks for 503 first, then other non-2xx errors.
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
// Policy CRUD
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PolicyCreateArgs {
    pub api_base: String,
    pub tenant_id: String,
    pub scope: String,
    pub window_secs: u64,
    pub max_count: Option<u64>,
    pub max_usd: Option<f64>,
    pub escalate_to: Option<String>,
}

pub async fn policy_create(args: PolicyCreateArgs) -> Result<JsonValue> {
    if args.max_count.is_none() && args.max_usd.is_none() {
        bail!("at least one of --max-count or --max-usd must be supplied");
    }
    let client = Client::new();
    let body = serde_json::json!({
        "tenant_id": args.tenant_id,
        "scope": args.scope,
        "window_seconds": args.window_secs,
        "max_count": args.max_count,
        "max_usd": args.max_usd,
        "escalate_to": args.escalate_to,
    });
    let resp = client
        .post(format!("{}/v1/hotl/policies", args.api_base))
        .json(&body)
        .send()
        .await
        .context("POST /v1/hotl/policies")?;
    let resp = require_ok(resp).await?;
    let v: JsonValue = resp.json().await.context("decode policy create body")?;
    Ok(v)
}

#[derive(Debug, Clone, Default)]
pub struct PolicyListArgs {
    pub api_base: String,
    pub tenant_id: String,
    pub scope: Option<String>,
}

pub async fn policy_list(args: PolicyListArgs) -> Result<Vec<JsonValue>> {
    let client = Client::new();
    let mut url = format!(
        "{}/v1/hotl/policies?tenant_id={}",
        args.api_base, args.tenant_id
    );
    if let Some(s) = &args.scope {
        url.push_str(&format!("&scope={s}"));
    }
    let resp = client
        .get(&url)
        .send()
        .await
        .context("GET /v1/hotl/policies")?;
    let resp = require_ok(resp).await?;
    let v: Vec<JsonValue> = resp.json().await.context("decode policy list body")?;
    Ok(v)
}

#[derive(Debug, Clone)]
pub struct PolicyGetArgs {
    pub api_base: String,
    pub id: String,
}

pub async fn policy_get(args: PolicyGetArgs) -> Result<JsonValue> {
    let client = Client::new();
    let resp = client
        .get(format!("{}/v1/hotl/policies/{}", args.api_base, args.id))
        .send()
        .await
        .context("GET /v1/hotl/policies/:id")?;
    let resp = require_ok(resp).await?;
    let v: JsonValue = resp.json().await.context("decode policy get body")?;
    Ok(v)
}

#[derive(Debug, Clone)]
pub struct PolicyUpdateArgs {
    pub api_base: String,
    pub id: String,
    pub max_count: Option<u64>,
    pub max_usd: Option<f64>,
    pub escalate_to: Option<String>,
    pub window_secs: Option<u64>,
}

pub async fn policy_update(args: PolicyUpdateArgs) -> Result<JsonValue> {
    let client = Client::new();
    let body = serde_json::json!({
        "max_count": args.max_count,
        "max_usd": args.max_usd,
        "escalate_to": args.escalate_to,
        "window_seconds": args.window_secs,
    });
    let resp = client
        .patch(format!("{}/v1/hotl/policies/{}", args.api_base, args.id))
        .json(&body)
        .send()
        .await
        .context("PATCH /v1/hotl/policies/:id")?;
    let resp = require_ok(resp).await?;
    let v: JsonValue = resp.json().await.context("decode policy update body")?;
    Ok(v)
}

#[derive(Debug, Clone)]
pub struct PolicyDeleteArgs {
    pub api_base: String,
    pub id: String,
}

pub async fn policy_delete(args: PolicyDeleteArgs) -> Result<()> {
    let client = Client::new();
    let resp = client
        .delete(format!("{}/v1/hotl/policies/{}", args.api_base, args.id))
        .send()
        .await
        .context("DELETE /v1/hotl/policies/:id")?;
    require_ok(resp).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Check
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CheckArgs {
    pub api_base: String,
    pub tenant_id: String,
    pub scope: String,
    pub amount: f64,
}

#[derive(Debug, Deserialize)]
pub struct CheckResponse {
    pub verdict: String,
    pub reason: Option<String>,
}

pub async fn check(args: CheckArgs) -> Result<CheckResponse> {
    let client = Client::new();
    let body = serde_json::json!({
        "tenant_id": args.tenant_id,
        "scope": args.scope,
        "amount": args.amount,
    });
    let resp = client
        .post(format!("{}/v1/hotl/check", args.api_base))
        .json(&body)
        .send()
        .await
        .context("POST /v1/hotl/check")?;
    let resp = require_ok(resp).await?;
    let v: CheckResponse = resp.json().await.context("decode check body")?;
    Ok(v)
}

// ---------------------------------------------------------------------------
// Table formatting
// ---------------------------------------------------------------------------

#[must_use]
pub fn format_policy_table(rows: &[JsonValue]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<38} {:<12} {:<8} {:<10} {:<10} ESCALATE_TO",
        "ID", "SCOPE", "WINDOW", "MAX_COUNT", "MAX_USD"
    );
    for p in rows {
        let id = p.get("id").and_then(JsonValue::as_str).unwrap_or("-");
        let scope = p.get("scope").and_then(JsonValue::as_str).unwrap_or("-");
        let window = p
            .get("window_seconds")
            .and_then(JsonValue::as_u64)
            .map_or_else(|| "-".to_string(), |n| format!("{n} s"));
        let max_count = p
            .get("max_count")
            .and_then(JsonValue::as_u64)
            .map_or_else(|| "-".to_string(), |n| n.to_string());
        let max_usd = p
            .get("max_usd")
            .and_then(JsonValue::as_f64)
            .map_or_else(|| "-".to_string(), |f| format!("${f:.2}"));
        let escalate = p
            .get("escalate_to")
            .and_then(JsonValue::as_str)
            .unwrap_or("-");
        let _ = writeln!(
            out,
            "{id:<38} {scope:<12} {window:<8} {max_count:<10} {max_usd:<10} {escalate}"
        );
    }
    out
}
