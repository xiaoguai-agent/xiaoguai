//! `xiaoguai skills ...` — manage the skill-pack marketplace via the REST API.
//!
//! Talks to `GET /v1/skills/catalog`, `GET /v1/skills/installed`,
//! `POST /v1/skills/install`, and `DELETE /v1/skills/install/:id`.
//! On HTTP 503 prints a friendly message explaining that the Pg bridge ships
//! in v1.3.
//!
//! `install-from-file` is planned for v1.3; calling it currently exits with
//! an informative error.

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

#[derive(Debug, Clone)]
pub struct ListArgs {
    pub api_base: String,
    pub tenant_id: Option<String>,
    pub category: Option<String>,
    pub installed: bool,
}

pub async fn list(args: ListArgs) -> Result<Vec<JsonValue>> {
    let client = Client::new();
    if args.installed {
        let tenant = args
            .tenant_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("--tenant-id is required when --installed is set"))?;
        let url = format!("{}/v1/skills/installed?tenant_id={tenant}", args.api_base);
        let resp = client
            .get(&url)
            .send()
            .await
            .context("GET /v1/skills/installed")?;
        let resp = require_ok(resp).await?;
        let v: Vec<JsonValue> = resp.json().await.context("decode installed body")?;
        return Ok(v);
    }
    let mut url = format!("{}/v1/skills/catalog", args.api_base);
    let mut query_parts: Vec<String> = Vec::new();
    if let Some(cat) = &args.category {
        query_parts.push(format!("category={cat}"));
    }
    if let Some(tid) = &args.tenant_id {
        query_parts.push(format!("tenant_id={tid}"));
    }
    if !query_parts.is_empty() {
        url.push('?');
        url.push_str(&query_parts.join("&"));
    }
    let resp = client
        .get(&url)
        .send()
        .await
        .context("GET /v1/skills/catalog")?;
    let resp = require_ok(resp).await?;
    let v: Vec<JsonValue> = resp.json().await.context("decode catalog body")?;
    Ok(v)
}

// ---------------------------------------------------------------------------
// install
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct InstallArgs {
    pub api_base: String,
    pub tenant_id: String,
    pub pack: String,
    pub config: Option<String>,
}

pub async fn install(args: InstallArgs) -> Result<JsonValue> {
    let config_json: Option<JsonValue> = match &args.config {
        Some(s) => Some(serde_json::from_str(s).context("--config is not valid JSON")?),
        None => None,
    };
    let client = Client::new();
    let body = serde_json::json!({
        "tenant_id": args.tenant_id,
        "pack_slug": args.pack,
        "config": config_json,
    });
    let resp = client
        .post(format!("{}/v1/skills/install", args.api_base))
        .json(&body)
        .send()
        .await
        .context("POST /v1/skills/install")?;
    let resp = require_ok(resp).await?;
    let v: JsonValue = resp.json().await.context("decode install body")?;
    Ok(v)
}

// ---------------------------------------------------------------------------
// install-from-file  (planned for v1.3)
// ---------------------------------------------------------------------------

pub fn install_from_file_not_implemented() -> Result<()> {
    bail!(
        "install-from-file is planned for v1.3 (pack hot-reload loader not yet available). \
         Use `xg skills install --pack <SLUG>` to install a catalog pack."
    );
}

// ---------------------------------------------------------------------------
// uninstall
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct UninstallArgs {
    pub api_base: String,
    pub id: String,
}

pub async fn uninstall(args: UninstallArgs) -> Result<()> {
    let client = Client::new();
    let resp = client
        .delete(format!("{}/v1/skills/install/{}", args.api_base, args.id))
        .send()
        .await
        .context("DELETE /v1/skills/install/:id")?;
    require_ok(resp).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Table formatting
// ---------------------------------------------------------------------------

#[must_use]
pub fn format_catalog_table(rows: &[JsonValue]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<20} {:<28} {:<8} {:<10} DESCRIPTION",
        "SLUG", "NAME", "VERSION", "CATEGORY"
    );
    for r in rows {
        let slug = r.get("slug").and_then(JsonValue::as_str).unwrap_or("-");
        let name = r.get("name").and_then(JsonValue::as_str).unwrap_or("-");
        let version = r.get("version").and_then(JsonValue::as_str).unwrap_or("-");
        let category = r.get("category").and_then(JsonValue::as_str).unwrap_or("-");
        let desc = r
            .get("description")
            .and_then(JsonValue::as_str)
            .unwrap_or("-");
        let _ = writeln!(
            out,
            "{slug:<20} {name:<28} {version:<8} {category:<10} {desc}"
        );
    }
    out
}

#[must_use]
pub fn format_installed_table(rows: &[JsonValue]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<20} {:<18} {:<8} INSTALLED_AT",
        "ID", "PACK_SLUG", "VERSION"
    );
    for r in rows {
        let id = r.get("id").and_then(JsonValue::as_str).unwrap_or("-");
        let slug = r
            .get("pack_slug")
            .and_then(JsonValue::as_str)
            .unwrap_or("-");
        let version = r.get("version").and_then(JsonValue::as_str).unwrap_or("-");
        let ts = r
            .get("installed_at")
            .and_then(JsonValue::as_str)
            .unwrap_or("-");
        let _ = writeln!(out, "{id:<20} {slug:<18} {version:<8} {ts}");
    }
    out
}
