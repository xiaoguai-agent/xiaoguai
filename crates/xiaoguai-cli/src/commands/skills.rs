//! `xiaoguai skills ...` — manage the skill-pack marketplace via the REST API.
//!
//! Talks to `GET /v1/skills/catalog`, `GET /v1/skills/installed`,
//! `POST /v1/skills/install`, and `DELETE /v1/skills/install/:id`.
//! On HTTP 503 prints a friendly message noting the skill-packs subsystem is
//! not enabled on the server.
//!
//! `install-from-file` is planned for v1.3; calling it currently exits with
//! an informative error.

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde_json::Value as JsonValue;

const ERR_503: &str = "Endpoint returned 503 — the skill-packs subsystem is not enabled on this \
                       server. Check that `xiaoguai serve` is running and skill packs are \
                       configured.";

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
    pub category: Option<String>,
    pub installed: bool,
}

pub async fn list(args: ListArgs) -> Result<Vec<JsonValue>> {
    let client = Client::new();
    if args.installed {
        let url = format!("{}/v1/skills/installed", args.api_base);
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
    if let Some(cat) = &args.category {
        url.push_str(&format!("?category={cat}"));
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

// ---------------------------------------------------------------------------
// proposals (Tier-2 D.1)
// ---------------------------------------------------------------------------

/// List agent-authored skill proposals.
pub async fn proposals_list(api_base: &str, status: Option<&str>) -> Result<Vec<JsonValue>> {
    let mut url = format!("{api_base}/v1/skills/proposals");
    if let Some(s) = status {
        url.push_str("?status=");
        url.push_str(s);
    }
    let client = Client::new();
    let resp = client
        .get(&url)
        .send()
        .await
        .context("GET /v1/skills/proposals")?;
    let resp = require_ok(resp).await?;
    let v: Vec<JsonValue> = resp.json().await.context("decode proposals body")?;
    Ok(v)
}

/// Approve a proposal — server flips it to `installed` and writes the
/// YAML manifest into `~/.xiaoguai/skills/`.
pub async fn proposals_approve(api_base: &str, id: &str, decided_by: &str) -> Result<JsonValue> {
    let client = Client::new();
    let body = serde_json::json!({ "decided_by": decided_by });
    let resp = client
        .post(format!("{api_base}/v1/skills/proposals/{id}/approve"))
        .json(&body)
        .send()
        .await
        .context("POST /v1/skills/proposals/:id/approve")?;
    let resp = require_ok(resp).await?;
    let v: JsonValue = resp.json().await.context("decode approve body")?;
    Ok(v)
}

/// Reject a proposal with a human-readable reason.
pub async fn proposals_reject(
    api_base: &str,
    id: &str,
    decided_by: &str,
    reason: &str,
) -> Result<JsonValue> {
    let client = Client::new();
    let body = serde_json::json!({ "decided_by": decided_by, "reason": reason });
    let resp = client
        .post(format!("{api_base}/v1/skills/proposals/{id}/reject"))
        .json(&body)
        .send()
        .await
        .context("POST /v1/skills/proposals/:id/reject")?;
    let resp = require_ok(resp).await?;
    let v: JsonValue = resp.json().await.context("decode reject body")?;
    Ok(v)
}

#[must_use]
pub fn format_proposals_table(rows: &[JsonValue]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<24} {:<20} {:<8} {:<10} CREATED_AT",
        "ID", "NAME", "VERSION", "STATUS"
    );
    for r in rows {
        let id = r.get("id").and_then(JsonValue::as_str).unwrap_or("-");
        let name = r
            .pointer("/manifest/name")
            .and_then(JsonValue::as_str)
            .unwrap_or("-");
        let version = r
            .pointer("/manifest/version")
            .and_then(JsonValue::as_str)
            .unwrap_or("-");
        let status = r.get("status").and_then(JsonValue::as_str).unwrap_or("-");
        let ts = r
            .get("created_at")
            .and_then(JsonValue::as_str)
            .unwrap_or("-");
        let _ = writeln!(out, "{id:<24} {name:<20} {version:<8} {status:<10} {ts}");
    }
    out
}
