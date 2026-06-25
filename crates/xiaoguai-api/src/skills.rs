//! v1.2.28 — skill pack marketplace.
//!
//! Three endpoints under `/v1/skills`:
//!
//! * `GET  /v1/skills/catalog`            — list all available packs (static,
//!   read from `catalog/skill_packs.json` baked into the binary).
//! * `GET  /v1/skills/installed`          — list installed packs,
//!   backed by [`SkillPackRepository`].
//! * `POST /v1/skills/install`            — install a pack.
//!   Records a row in `installed_skill_packs`; does NOT hot-reload the pack
//!   (pack runtime integration is S1's pack-loader, tracked post-v1.2).
//! * `DELETE /v1/skills/install/:id`      — uninstall (soft-delete the row).
//!
//! The catalog file shape mirrors `mcp_marketplace.json` but adds `requires`,
//! `knobs`, and `screenshot_url` fields for the chat-ui Skills pane.

use std::collections::HashMap;
use std::sync::OnceLock;

use async_trait::async_trait;
use axum::extract::{Path, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Catalog file types (baked into binary at compile time)
// ---------------------------------------------------------------------------

const CATALOG_JSON: &str = include_str!("../../../catalog/skill_packs.json");

/// One knob definition from the catalog — JSON-schema-lite so the UI can
/// render a typed form without downloading a schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KnobSchema {
    Integer {
        default: i64,
        description: String,
    },
    /// Floating-point knob (e.g. a 0.0–1.0 threshold).
    Number {
        default: f64,
        description: String,
    },
    Boolean {
        default: bool,
        description: String,
    },
    /// Freeform or enum string.
    String {
        #[serde(default)]
        r#enum: Vec<String>,
        default: String,
        description: String,
    },
}

/// Feature-flag / env-var prerequisites the operator must satisfy before
/// installing a pack.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PackRequires {
    /// Names of xiaoguai feature flags that must be enabled
    /// (e.g. `"rag"`, `"scheduler"`).
    #[serde(default)]
    pub feature_flags: Vec<String>,
    /// Env-var names the pack's runtime tools will need at spawn time.
    #[serde(default)]
    pub env_keys: Vec<String>,
}

/// IA tiers for the Skills page (the `tier` field). `"general"` packs —
/// broadly-useful skills like document authoring or `superpowers` — render
/// first; `"specialized"` packs — domain scenario teams — sit behind a tab.
fn default_tier() -> String {
    "specialized".to_string()
}

/// One entry in `catalog/skill_packs.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillPackEntry {
    pub slug: String,
    pub name: String,
    /// Chinese display name. The UI prefers this when the locale is Chinese,
    /// falling back to `name`. Optional so a half-localized catalog still
    /// parses; the `all_entries_have_required_fields` test keeps it filled.
    #[serde(default)]
    pub name_zh: Option<String>,
    pub description: String,
    /// Chinese description, same fallback contract as `name_zh`.
    #[serde(default)]
    pub description_zh: Option<String>,
    pub version: String,
    /// Grouping hint for the UI: `"finance"`, `"ops"`, `"dev"`, `"hr"`,
    /// `"rag"`, `"documents"`, etc.
    pub category: String,
    /// IA tier — `"general"` (shown first) or `"specialized"` (behind a tab).
    /// Defaults to `"specialized"` so an un-tagged entry never crowds the
    /// general list. See [`default_tier`].
    #[serde(default = "default_tier")]
    pub tier: String,
    #[serde(default)]
    pub requires: PackRequires,
    /// Operator-tuneable knobs — the UI renders these as a typed form.
    #[serde(default)]
    pub knobs: HashMap<String, KnobSchema>,
    #[serde(default)]
    pub screenshot_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogFile {
    pub version: u32,
    pub packs: Vec<SkillPackEntry>,
}

fn catalog() -> &'static CatalogFile {
    static CATALOG: OnceLock<CatalogFile> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(CATALOG_JSON)
            .expect("catalog/skill_packs.json must parse — fix the file")
    })
}

// ---------------------------------------------------------------------------
// Wire types for the installed-packs API
// ---------------------------------------------------------------------------

/// Installed-pack storage row (DB shape). The API presents it as
/// [`InstalledSkillPackResponse`] via [`to_installed_response`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InstalledPackRow {
    pub id: String,
    pub pack_slug: String,
    pub version: String,
    /// Operator-supplied knob overrides stored as free-form JSON.
    pub config: serde_json::Value,
    pub installed_at: DateTime<Utc>,
}

/// Wire shape for `GET /v1/skills/installed` + `POST /v1/skills/install` —
/// matches the chat-ui/admin-ui `InstalledSkillPackResponse`. `pack_id` is the
/// stored `pack_slug`; `name`/`description` come from the baked catalog.
/// `agents`/`inbound_adapters`/`outputs` stay empty until the runtime pack
/// loader (post-v1.2) parses pack manifests, and `activation_status` is always
/// `"pending"` until then.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct InstalledSkillPackResponse {
    pub id: String,
    pub pack_id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub agents: Vec<String>,
    pub inbound_adapters: Vec<String>,
    pub outputs: Vec<String>,
    pub recorded_at: DateTime<Utc>,
    pub activation_status: &'static str,
}

/// Map a stored row to the API response, enriching `name`/`description` from
/// the catalog (falling back to the slug when the pack is no longer listed).
fn to_installed_response(row: InstalledPackRow) -> InstalledSkillPackResponse {
    let entry = catalog().packs.iter().find(|p| p.slug == row.pack_slug);
    // Phase 4b: the serve boot-scan records the activated agent names in the
    // pack's `config` when it upserts the conversational team. Their presence
    // flips the pack to "active" and lists them; absence ⇒ still "pending".
    let agents: Vec<String> = row
        .config
        .get("agents")
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let activation_status = if agents.is_empty() {
        "pending"
    } else {
        "active"
    };
    InstalledSkillPackResponse {
        id: row.id,
        name: entry.map_or_else(|| row.pack_slug.clone(), |e| e.name.clone()),
        description: entry.map(|e| e.description.clone()),
        pack_id: row.pack_slug,
        version: row.version,
        agents,
        inbound_adapters: Vec::new(),
        outputs: Vec::new(),
        recorded_at: row.installed_at,
        activation_status,
    }
}

/// Repository trait — production impl will be a `SqliteSkillPackRepository` in
/// `xiaoguai-core`; tests use [`InMemorySkillPackRepository`].
#[async_trait]
pub trait SkillPackRepository: Send + Sync {
    /// Return all installed packs.
    async fn list(&self) -> Result<Vec<InstalledPackRow>, SkillPackError>;

    /// Insert a new row. Returns `Err(SkillPackError::AlreadyInstalled)` when
    /// the `UNIQUE (pack_slug)` constraint fires.
    async fn install(&self, row: InstalledPackRow) -> Result<InstalledPackRow, SkillPackError>;

    /// Delete a row by `id`. Returns `Err(SkillPackError::NotFound)` when the
    /// row doesn't exist.
    async fn uninstall(&self, id: &str) -> Result<(), SkillPackError>;
}

#[derive(Debug, Clone, Error)]
pub enum SkillPackError {
    #[error("pack already installed")]
    AlreadyInstalled,
    #[error("not found")]
    NotFound,
    #[error("backend error: {0}")]
    Backend(String),
}

// ---------------------------------------------------------------------------
// In-memory test implementation
// ---------------------------------------------------------------------------

use parking_lot::Mutex;
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct InMemorySkillPackRepository {
    rows: Mutex<Vec<InstalledPackRow>>,
}

impl InMemorySkillPackRepository {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

#[async_trait]
impl SkillPackRepository for InMemorySkillPackRepository {
    async fn list(&self) -> Result<Vec<InstalledPackRow>, SkillPackError> {
        let rows = self.rows.lock();
        Ok(rows.iter().cloned().collect())
    }

    async fn install(&self, row: InstalledPackRow) -> Result<InstalledPackRow, SkillPackError> {
        let mut rows = self.rows.lock();
        let dup = rows.iter().any(|r| r.pack_slug == row.pack_slug);
        if dup {
            return Err(SkillPackError::AlreadyInstalled);
        }
        rows.push(row.clone());
        Ok(row)
    }

    async fn uninstall(&self, id: &str) -> Result<(), SkillPackError> {
        let mut rows = self.rows.lock();
        let before = rows.len();
        rows.retain(|r| r.id != id);
        if rows.len() == before {
            Err(SkillPackError::NotFound)
        } else {
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Route handlers
// ---------------------------------------------------------------------------

/// `GET /v1/skills/catalog`
///
/// Returns the full catalog baked into the binary. No auth — callers
/// can browse available packs without credentials.
///
/// # Errors
/// Returns an error if the catalog file cannot be parsed.
#[allow(clippy::unused_async)]
pub async fn list_catalog() -> ApiResult<Json<CatalogFile>> {
    Ok(Json(catalog().clone()))
}

/// `GET /v1/skills/installed`
///
/// # Errors
/// Returns an error if the repository fails.
pub async fn list_installed(
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<InstalledSkillPackResponse>>> {
    let repo = skill_repo(&state)?;
    let rows = repo.list().await.map_err(skill_err_to_api)?;
    Ok(Json(rows.into_iter().map(to_installed_response).collect()))
}

#[derive(Debug, Deserialize)]
pub struct InstallRequest {
    /// Pack identifier (catalog slug). Accepts the legacy `pack_slug` key too,
    /// so older clients keep working.
    #[serde(alias = "pack_slug")]
    pub pack_id: String,
    /// Operator-supplied knob overrides. Validated against the catalog schema
    /// in a best-effort manner — unknown keys are accepted (forward compat).
    #[serde(default)]
    pub config: serde_json::Value,
}

/// `POST /v1/skills/install`
///
/// # Errors
/// Returns an error if the pack slug is not found in the catalog or the repository fails.
pub async fn install_pack(
    State(state): State<AppState>,
    Json(req): Json<InstallRequest>,
) -> ApiResult<Json<InstalledSkillPackResponse>> {
    let repo = skill_repo(&state)?;

    // Verify the slug is in the catalog so we don't record phantom installs.
    let entry = catalog()
        .packs
        .iter()
        .find(|p| p.slug == req.pack_id)
        .ok_or(ApiError::NotFound)?;

    let row = InstalledPackRow {
        id: Uuid::new_v4().to_string(),
        pack_slug: req.pack_id,
        version: entry.version.clone(),
        config: req.config,
        installed_at: Utc::now(),
    };

    let saved = repo.install(row).await.map_err(|e| match e {
        SkillPackError::AlreadyInstalled => ApiError::Conflict("pack already installed".into()),
        other => skill_err_to_api(other),
    })?;

    Ok(Json(to_installed_response(saved)))
}

/// `DELETE /v1/skills/install/:id`
///
/// # Errors
/// Returns an error if the pack is not found or the repository fails.
pub async fn uninstall_pack(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let repo = skill_repo(&state)?;
    repo.uninstall(&id).await.map_err(|e| match e {
        SkillPackError::NotFound => ApiError::NotFound,
        other => skill_err_to_api(other),
    })?;
    Ok(Json(serde_json::json!({ "deleted": id })))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn skill_repo(state: &AppState) -> ApiResult<Arc<dyn SkillPackRepository>> {
    state
        .skill_packs
        .clone()
        .ok_or_else(|| ApiError::ServiceUnavailable("skill pack repository not wired".into()))
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "error is moved into anyhow for ownership"
)]
fn skill_err_to_api(e: SkillPackError) -> ApiError {
    ApiError::Internal(anyhow::anyhow!("skill pack store: {e}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- catalog-level unit tests -------------------------------------------

    #[test]
    fn catalog_parses_and_has_expected_slugs() {
        let c = catalog();
        assert_eq!(c.version, 1);
        let slugs: Vec<&str> = c.packs.iter().map(|p| p.slug.as_str()).collect();
        for expected in &[
            "ar-collections",
            "incident-triage",
            "pr-review",
            "hr-onboarding",
            "rag-legal",
            "rag-finance",
            "rag-hr",
            "code-review",
            "doc-qa",
            "sql-assistant",
            "customer-support",
            "meeting-notes",
            "release-notes",
        ] {
            assert!(slugs.contains(expected), "catalog missing slug: {expected}");
        }
    }

    #[test]
    fn all_catalog_slugs_unique() {
        let c = catalog();
        let mut seen = std::collections::HashSet::new();
        for p in &c.packs {
            assert!(seen.insert(&p.slug), "duplicate slug: {}", p.slug);
        }
    }

    #[test]
    fn all_entries_have_required_fields() {
        let c = catalog();
        for p in &c.packs {
            assert!(!p.slug.is_empty(), "slug must not be empty");
            assert!(!p.name.is_empty(), "name must not be empty: {}", p.slug);
            assert!(
                !p.description.is_empty(),
                "description must not be empty: {}",
                p.slug
            );
            assert!(
                !p.version.is_empty(),
                "version must not be empty: {}",
                p.slug
            );
            assert!(
                !p.category.is_empty(),
                "category must not be empty: {}",
                p.slug
            );
            // Bilingual contract: every entry MUST carry a non-empty Chinese
            // name + description so the Skills page never falls back to English
            // under a Chinese locale (the regression this catalog fixes).
            assert!(
                p.name_zh.as_deref().is_some_and(|s| !s.is_empty()),
                "name_zh must be present and non-empty: {}",
                p.slug
            );
            assert!(
                p.description_zh.as_deref().is_some_and(|s| !s.is_empty()),
                "description_zh must be present and non-empty: {}",
                p.slug
            );
            assert!(
                p.tier == "general" || p.tier == "specialized",
                "tier must be \"general\" or \"specialized\": {} (got {:?})",
                p.slug,
                p.tier
            );
        }
    }

    // --- InMemorySkillPackRepository tests -----------------------------------

    #[tokio::test]
    async fn install_round_trip() {
        let repo = InMemorySkillPackRepository::new();
        let row = InstalledPackRow {
            id: Uuid::new_v4().to_string(),
            pack_slug: "rag-hr".into(),
            version: "1.0.0".into(),
            config: serde_json::json!({"top_k": 10}),
            installed_at: Utc::now(),
        };
        let saved = repo.install(row.clone()).await.unwrap();
        assert_eq!(saved.pack_slug, "rag-hr");

        let listed = repo.list().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, row.id);
    }

    #[tokio::test]
    async fn duplicate_install_rejected() {
        let repo = InMemorySkillPackRepository::new();
        let make = || InstalledPackRow {
            id: Uuid::new_v4().to_string(),
            pack_slug: "pr-review".into(),
            version: "1.0.0".into(),
            config: serde_json::json!({}),
            installed_at: Utc::now(),
        };
        repo.install(make()).await.unwrap();
        let err = repo.install(make()).await.unwrap_err();
        assert!(matches!(err, SkillPackError::AlreadyInstalled));
    }

    #[tokio::test]
    async fn uninstall_round_trip() {
        let repo = InMemorySkillPackRepository::new();
        let row = InstalledPackRow {
            id: Uuid::new_v4().to_string(),
            pack_slug: "hr-onboarding".into(),
            version: "1.0.0".into(),
            config: serde_json::json!({}),
            installed_at: Utc::now(),
        };
        let saved = repo.install(row.clone()).await.unwrap();
        assert_eq!(repo.list().await.unwrap().len(), 1);

        repo.uninstall(&saved.id).await.unwrap();
        assert!(repo.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn uninstall_unknown_id_is_not_found() {
        let repo = InMemorySkillPackRepository::new();
        let err = repo.uninstall("does-not-exist").await.unwrap_err();
        assert!(matches!(err, SkillPackError::NotFound));
    }
}
// HTTP integration tests live in tests/skills.rs (uses the common module).
