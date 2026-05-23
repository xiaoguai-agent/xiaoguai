//! v0.9.4 — curated MCP marketplace.
//!
//! Two endpoints:
//!
//! * `GET  /v1/mcp/marketplace` — returns the static catalog of
//!   recommended MCP servers. The catalog ships as a versioned JSON
//!   blob baked into the binary (no marketplace backend, no remote
//!   fetch — operators audit a single file).
//! * `POST /v1/mcp/marketplace/install` — body `{slug, tenant_id?}`.
//!   Looks up the slug in the catalog, materialises an `McpServer`
//!   row, and writes it via `McpServerRepository::create`. Returns
//!   the created row.
//!
//! Roadmap principle: "Skill of the platform is in curation, not in
//! hosting." We don't run a marketplace; we ship a tasteful default.

use std::sync::OnceLock;

use axum::extract::State;
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use xiaoguai_types::{ids::McpServerInstanceId, McpServer, McpTransport, TenantId};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

const CATALOG_JSON: &str = include_str!("../catalog/mcp_marketplace.json");

/// One marketplace entry — what the operator sees + enough metadata
/// to materialise an `mcp_servers` row on install. The JSON file
/// matches this shape; `serde_json::from_str` will fail loud at
/// startup-touch if it drifts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceEntry {
    /// Stable slug, used as the install key. Lowercase, hyphen-separated.
    pub slug: String,
    pub name: String,
    pub description: String,
    /// Free-form one of: `files`, `code`, `notes`, `chat`, `data`,
    /// `email`. UI uses it for grouping.
    pub category: String,
    pub transport: String,
    pub version: String,
    /// stdio: command + args. http/sse: ignored.
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// http/sse: endpoint URL. stdio: ignored.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Names (not values) of env vars the server expects at spawn
    /// time. Operators populate them out-of-band.
    #[serde(default)]
    pub env_keys: Vec<String>,
    /// Optional anchor URL to the upstream project.
    #[serde(default)]
    pub source_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MarketplaceResponse {
    pub version: u32,
    pub entries: Vec<MarketplaceEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct CatalogFile {
    version: u32,
    entries: Vec<MarketplaceEntry>,
}

fn catalog() -> &'static CatalogFile {
    static CATALOG: OnceLock<CatalogFile> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(CATALOG_JSON)
            .expect("catalog/mcp_marketplace.json must parse — fix the file")
    })
}

/// `GET /v1/mcp/marketplace`.
#[allow(clippy::unused_async)]
pub async fn list_marketplace() -> ApiResult<Json<MarketplaceResponse>> {
    let c = catalog();
    Ok(Json(MarketplaceResponse {
        version: c.version,
        entries: c.entries.clone(),
    }))
}

#[derive(Debug, Deserialize)]
pub struct InstallRequest {
    pub slug: String,
    /// `None` ⇒ install as a global (system-wide) MCP server. Mirrors
    /// the existing `McpServer.tenant_id` semantics — `None` is shared
    /// across every tenant.
    #[serde(default)]
    pub tenant_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InstallResponse {
    pub id: String,
    pub slug: String,
    pub name: String,
}

/// `POST /v1/mcp/marketplace/install`.
pub async fn install_from_marketplace(
    State(state): State<AppState>,
    Json(req): Json<InstallRequest>,
) -> ApiResult<Json<InstallResponse>> {
    let repo = state.mcp_servers.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("MCP server repository not wired into AppState".into())
    })?;

    let entry = catalog()
        .entries
        .iter()
        .find(|e| e.slug == req.slug)
        .ok_or_else(|| ApiError::NotFound)?;

    let transport = McpTransport::parse(&entry.transport).ok_or_else(|| {
        ApiError::InvalidRequest(format!(
            "catalog entry {} has unknown transport: {}",
            entry.slug, entry.transport
        ))
    })?;

    let now = Utc::now();
    let server = McpServer {
        id: McpServerInstanceId::new(),
        tenant_id: req.tenant_id.clone().map(TenantId::from),
        name: entry.name.clone(),
        version: entry.version.clone(),
        transport,
        command: entry.command.clone(),
        args: entry.args.clone(),
        env_keys: entry.env_keys.clone(),
        endpoint: entry.endpoint.clone(),
        enabled: true,
        created_at: now,
        updated_at: now,
    };

    repo.create(req.tenant_id.as_deref(), &server).await?;

    // v0.9.4.1: live-pickup. If the operator wired a supervisor, ask it
    // to reconcile against the row we just wrote so the newly installed
    // server is reachable without a process restart. Best-effort: a
    // spawn failure (missing binary on PATH, env var unset) is logged
    // but doesn't fail the install — the DB row is the source of truth.
    if let Some(sup) = state.mcp_supervisor.as_ref() {
        match sup
            .reload_from_db(repo.as_ref(), req.tenant_id.as_deref())
            .await
        {
            Ok(started) => {
                if !started.is_empty() {
                    tracing::info!(
                        slug = %req.slug,
                        started = ?started,
                        "mcp marketplace install: supervisor picked up new servers",
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    slug = %req.slug,
                    error = %e,
                    "mcp marketplace install: supervisor reload failed",
                );
            }
        }
    }

    Ok(Json(InstallResponse {
        id: server.id.to_string(),
        slug: entry.slug.clone(),
        name: entry.name.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_parses() {
        let c = catalog();
        assert!(c.version >= 1);
        assert!(
            !c.entries.is_empty(),
            "marketplace must ship at least one entry"
        );
    }

    #[test]
    fn every_entry_has_compatible_transport() {
        for e in &catalog().entries {
            assert!(
                McpTransport::parse(&e.transport).is_some(),
                "entry {} has unknown transport {}",
                e.slug,
                e.transport
            );
            match e.transport.as_str() {
                "stdio" => assert!(
                    e.command.is_some(),
                    "stdio entry {} must specify a command",
                    e.slug
                ),
                "http" | "sse" => assert!(
                    e.endpoint.is_some(),
                    "{} entry {} must specify an endpoint",
                    e.transport,
                    e.slug
                ),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn slugs_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for e in &catalog().entries {
            assert!(seen.insert(&e.slug), "duplicate slug: {}", e.slug);
        }
    }
}
