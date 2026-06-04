//! Tier-2 D.1 — Agent-authored skills (HotL-gated, admin-approved).
//!
//! An agent can author a new skill-pack manifest at runtime through the
//! `propose_skill` MCP tool registered in `xiaoguai-agent`. The dispatch
//! lands here:
//!
//! 1. [`propose`] checks the opt-in flag (off by default).
//! 2. The manifest is validated against a strict whitelist schema —
//!    agent-authored manifests reference existing tools by name; they
//!    CANNOT declare new MCP servers or load native code.
//! 3. The `HotL` gate (PR #61) is consulted under bucket `skill_author`
//!    with a default budget of 5 proposals / day.
//! 4. On `Allow` the manifest is persisted as `pending` in
//!    `skill_proposals`; on `Deny` the reason is fed back to the LLM.
//! 5. A human admin later calls [`approve_proposal`] (wrapped by an HTTP
//!    endpoint in `xiaoguai-api`) which flips the row to `installed` and
//!    writes the manifest as YAML to `~/.xiaoguai/skills/`.
//!
//! The flow emits three audit-log rows: `skill.propose`, `skill.hotl_gate`,
//! `skill.approve` — together they form a tamper-evident chain of
//! everything an agent authored.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use xiaoguai_audit::{AuditEntry, OWNER_TENANT_ID};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// Strict whitelist schema for agent-authored skill manifests.
///
/// Any field outside this shape is rejected by [`validate_manifest`]. The
/// JSON-schema rendered for the MCP tool also pins `additionalProperties:
/// false` so the LLM can't pad the payload with extra keys.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillManifest {
    pub name: String,
    pub description: String,
    pub version: String,
    pub system_prompt: String,
    /// Names of tools the skill is allowed to call. MUST be a subset of
    /// the tools registered in the running agent's toolbox at proposal
    /// time. Cannot include the meta-tool `propose_skill` (no recursion).
    pub tool_allowlist: Vec<String>,
}

/// Lifecycle states a proposal moves through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalStatus {
    Pending,
    Approved,
    Rejected,
    Installed,
}

impl ProposalStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Installed => "installed",
        }
    }

    /// Parse the database string back into the enum. Unknown strings
    /// produce `None` so the caller can decide between 500 and silent
    /// skip.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "approved" => Some(Self::Approved),
            "rejected" => Some(Self::Rejected),
            "installed" => Some(Self::Installed),
            _ => None,
        }
    }
}

/// One row in `skill_proposals`. The persistence layer round-trips this
/// shape; the HTTP layer renders it as JSON; the YAML emitter (on
/// approval) walks `manifest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalRow {
    pub id: String,
    pub proposed_by: String,
    pub manifest: SkillManifest,
    pub status: ProposalStatus,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub decided_by: Option<String>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SkillAuthorError {
    /// The owner has not opted in to agent-authored skills. The propose
    /// path returns this *before* any audit emission so off-by-default
    /// is also off-the-record.
    #[error("agent-authored skills are not enabled")]
    Disabled,
    /// Manifest failed the whitelist validator.
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),
    /// `HotL` gate returned `Deny`.
    #[error("hotl gate denied: {0}")]
    Denied(String),
    /// Proposal coordinates clash with an existing row.
    #[error("a proposal with this name and version already exists")]
    Duplicate,
    /// Lookup or state-transition target missing.
    #[error("proposal not found")]
    NotFound,
    /// Manifest could not be rendered to YAML.
    #[error("yaml render failed: {0}")]
    YamlRender(String),
    /// Manifest file already exists on disk at approval time.
    #[error("skill file already exists on disk")]
    SkillFileExists,
    /// Repository / storage / IO failure.
    #[error("backend error: {0}")]
    Backend(String),
}

// ---------------------------------------------------------------------------
// Traits — repository, settings, audit, gate adapter
// ---------------------------------------------------------------------------

/// Persistence seam for `skill_proposals`. Production wires a `SQLite`
/// impl from `xiaoguai-tasks::pg`; tests use [`InMemorySkillProposalRepository`].
#[async_trait]
pub trait SkillProposalRepository: Send + Sync {
    async fn insert(&self, row: ProposalRow) -> Result<ProposalRow, SkillAuthorError>;
    async fn get(&self, id: &str) -> Result<Option<ProposalRow>, SkillAuthorError>;
    async fn list(
        &self,
        status: Option<ProposalStatus>,
    ) -> Result<Vec<ProposalRow>, SkillAuthorError>;
    async fn set_status(
        &self,
        id: &str,
        status: ProposalStatus,
        decided_by: &str,
        reason: Option<&str>,
    ) -> Result<ProposalRow, SkillAuthorError>;
}

/// Owner opt-in flag store. Backed by `tenant_settings` JSONB.
#[async_trait]
pub trait TenantSettingsReader: Send + Sync {
    async fn allow_skill_authoring(&self) -> Result<bool, SkillAuthorError>;

    /// DEC-019: returns the sandbox tier the operator wants. Used by the
    /// MCP supervisor in `xiaoguai-core` to decide which exec server
    /// binary to spawn (`xiaoguai-mcp-exec` for L1 vs
    /// `xiaoguai-mcp-exec-wasm-py` for L3). Defaults to
    /// [`SandboxTier::L1`] when there is no explicit setting —
    /// L1 is safe-by-default per PHILO §14.
    async fn sandbox_tier(&self) -> Result<SandboxTier, SkillAuthorError>;
}

/// Sandbox tier for code-execution MCP servers (DEC-019). Operators set
/// this via `tenant_settings.settings->>'sandbox_tier'`.
///
/// L1 is the process-isolated default (`xiaoguai-mcp-exec` +
/// `xiaoguai-mcp-exec-js`); L3 is the wasmtime capability sandbox
/// (`xiaoguai-mcp-exec-wasm-py` + `xiaoguai-mcp-exec-wasm-js`,
/// DEC-020). L2 (container) and L4 (full VM) are not supported by this
/// enum — operators wrap L1/L3 binaries in containers themselves if
/// needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxTier {
    L1,
    L3,
}

impl SandboxTier {
    /// Parse the stored string ("L1" / "L3"; case-insensitive).
    /// Unknown values fall back to L1 (safe default per PHILO §14).
    #[must_use]
    pub fn from_str_lenient(s: &str) -> Self {
        match s.trim().to_ascii_uppercase().as_str() {
            "L3" => Self::L3,
            _ => Self::L1,
        }
    }

    /// Stable tier label for logs and metrics.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::L1 => "L1",
            Self::L3 => "L3",
        }
    }
}

/// `HotL` gate adapter used by this module. Mirrors
/// `xiaoguai_agent::hotl_gate::HotlGate` but kept here to avoid a hard
/// dependency on `xiaoguai-agent` (which would create a crate-graph
/// cycle since agent already depends on a future tasks-bridge in core).
///
/// `xiaoguai-core` provides an adapter mapping the real gate onto this
/// trait. Tests use [`AllowAllSkillGate`] / [`DenySkillGate`].
#[async_trait]
pub trait SkillAuthorGate: Send + Sync {
    /// Returns `Ok(())` on Allow, `Err(reason)` on Deny / infra failure
    /// (fail-closed, matches PR #61 semantics).
    async fn check(&self, scope: &str) -> Result<(), String>;
}

/// Audit sink seam — single-call surface so test fixtures stay tiny.
/// Production wires a thin adapter over `SqliteAuditSink::append` from
/// `xiaoguai-audit`; tests use [`InMemoryAuditSink`].
#[async_trait]
pub trait SkillAuditSink: Send + Sync {
    async fn record(&self, entry: AuditEntry) -> Result<(), SkillAuthorError>;
}

// ---------------------------------------------------------------------------
// Context bundle passed into propose / approve
// ---------------------------------------------------------------------------

/// References to the collaborators `propose` / `approve_proposal` need.
/// Bundled into a struct so callers don't push 6 positional args around.
pub struct SkillAuthorCtx<'a> {
    pub repo: &'a (dyn SkillProposalRepository + 'a),
    pub settings: &'a (dyn TenantSettingsReader + 'a),
    pub gate: &'a (dyn SkillAuthorGate + 'a),
    pub audit: &'a (dyn SkillAuditSink + 'a),
    /// Tool names the agent's toolbox exposes at proposal time. Used to
    /// validate `tool_allowlist`.
    pub known_tools: &'a HashSet<String>,
}

// ---------------------------------------------------------------------------
// Manifest validator
// ---------------------------------------------------------------------------

/// Reject manifests that violate the whitelist schema.
///
/// Rules:
/// * Name, description, version, `system_prompt`: non-empty after trim.
/// * Name: alphanumeric + `-`/`_` only (matches the existing
///   marketplace slug pattern).
/// * Version: SemVer-ish (`X.Y.Z` with optional `-pre`). Conservative —
///   we want predictable filenames.
/// * `tool_allowlist`: every entry MUST appear in `known_tools`, MUST NOT
///   contain `propose_skill` (recursion guard), MUST be non-empty (a
///   skill with no tools is a dead skill — likely an LLM hallucination).
/// * No other field exists on the struct (rust's type system already
///   enforces this) — the JSON-schema for the MCP tool also pins
///   `additionalProperties: false` so the LLM can't smuggle anything
///   past serde.
#[allow(clippy::implicit_hasher)] // public API; caller passes the default hasher.
pub fn validate_manifest(
    m: &SkillManifest,
    known_tools: &HashSet<String>,
) -> Result<(), SkillAuthorError> {
    fn nonempty(field: &str, v: &str) -> Result<(), SkillAuthorError> {
        if v.trim().is_empty() {
            return Err(SkillAuthorError::InvalidManifest(format!(
                "{field} must be non-empty"
            )));
        }
        Ok(())
    }

    nonempty("name", &m.name)?;
    nonempty("description", &m.description)?;
    nonempty("version", &m.version)?;
    nonempty("system_prompt", &m.system_prompt)?;

    if !m
        .name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(SkillAuthorError::InvalidManifest(
            "name must contain only alphanumeric, '-', or '_' characters".into(),
        ));
    }

    if !version_is_semver_ish(&m.version) {
        return Err(SkillAuthorError::InvalidManifest(format!(
            "version {:?} is not a valid X.Y.Z form",
            m.version
        )));
    }

    if m.tool_allowlist.is_empty() {
        return Err(SkillAuthorError::InvalidManifest(
            "tool_allowlist must contain at least one tool".into(),
        ));
    }

    for t in &m.tool_allowlist {
        if t == PROPOSE_SKILL_TOOL_NAME {
            return Err(SkillAuthorError::InvalidManifest(
                "tool_allowlist must not contain propose_skill (no recursion)".into(),
            ));
        }
        if !known_tools.contains(t) {
            return Err(SkillAuthorError::InvalidManifest(format!(
                "tool {t:?} is not registered in the current toolbox"
            )));
        }
    }

    Ok(())
}

/// Conservative `SemVer` check: three numeric segments separated by `.`,
/// optionally followed by `-<alphanumeric/-/.>`. We don't pull in the
/// `semver` crate for this one call.
fn version_is_semver_ish(v: &str) -> bool {
    let (core, _pre) = v.split_once('-').map_or((v, ""), |(a, b)| (a, b));
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3 {
        return false;
    }
    parts
        .iter()
        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// Canonical name of the tool that triggers `propose`. Used both by the
/// recursion-guard in [`validate_manifest`] and by the agent-side tool
/// descriptor.
pub const PROPOSE_SKILL_TOOL_NAME: &str = "propose_skill";

/// `HotL` gate scope passed by [`propose`].
pub const SKILL_AUTHOR_GATE_SCOPE: &str = "skill_author";

// ---------------------------------------------------------------------------
// Audit helpers
// ---------------------------------------------------------------------------

fn audit_entry(
    actor: &str,
    action: &'static str,
    resource: Option<String>,
    details: serde_json::Value,
) -> AuditEntry {
    AuditEntry {
        ts: Utc::now(),
        // Audit chain: sign with the owner identity that `verify_chain`
        // rebuilds with (single-owner model, DEC-033 / DEC-HLD-021).
        tenant_id: OWNER_TENANT_ID.to_string(),
        actor: actor.to_string(),
        action: action.to_string(),
        resource,
        details,
    }
}

// ---------------------------------------------------------------------------
// propose / approve_proposal — the two public entry points
// ---------------------------------------------------------------------------

/// Persist a new agent-authored proposal as `pending` after running it
/// through the gate. The full flow is described at the top of this
/// module.
pub async fn propose(
    ctx: &SkillAuthorCtx<'_>,
    proposed_by: &str,
    manifest: SkillManifest,
) -> Result<ProposalRow, SkillAuthorError> {
    // 1. Off-by-default check. Quiet drop — no audit row when disabled,
    //    so an enumeration attack can't be detected from the audit log.
    if !ctx.settings.allow_skill_authoring().await? {
        return Err(SkillAuthorError::Disabled);
    }

    // 2. Whitelist validation. Runs before gate so a malformed manifest
    //    doesn't burn the daily budget.
    validate_manifest(&manifest, ctx.known_tools)?;

    // 3. Audit emit `skill.propose` BEFORE the gate so we have a record
    //    even if the gate denies.
    ctx.audit
        .record(audit_entry(
            proposed_by,
            "skill.propose",
            Some(format!("skill:{}@{}", manifest.name, manifest.version)),
            serde_json::json!({
                "name": manifest.name,
                "version": manifest.version,
                "tool_allowlist": manifest.tool_allowlist,
            }),
        ))
        .await?;

    // 4. HotL gate consultation.
    let gate_outcome = ctx.gate.check(SKILL_AUTHOR_GATE_SCOPE).await;

    let (verdict_str, reason_opt) = match &gate_outcome {
        Ok(()) => ("allow", None),
        Err(r) => ("deny", Some(r.clone())),
    };
    ctx.audit
        .record(audit_entry(
            proposed_by,
            "skill.hotl_gate",
            Some(format!("skill:{}@{}", manifest.name, manifest.version)),
            serde_json::json!({
                "scope": SKILL_AUTHOR_GATE_SCOPE,
                "verdict": verdict_str,
                "reason": reason_opt,
            }),
        ))
        .await?;

    if let Err(reason) = gate_outcome {
        return Err(SkillAuthorError::Denied(reason));
    }

    // 5. Insert the row.
    let row = ProposalRow {
        id: Uuid::new_v4().to_string(),
        proposed_by: proposed_by.to_string(),
        manifest,
        status: ProposalStatus::Pending,
        reason: None,
        created_at: Utc::now(),
        decided_at: None,
        decided_by: None,
    };
    ctx.repo.insert(row).await
}

/// Admin-triggered approval. Flips the row to `installed` and writes the
/// manifest as YAML to `<skills_dir>/<name>-<version>.yaml`.
///
/// Returns the updated row on success. On YAML write failure the DB row
/// is rolled back to `pending` so we don't end up with an `installed`
/// row without a file on disk.
pub async fn approve_proposal(
    ctx: &SkillAuthorCtx<'_>,
    proposal_id: &str,
    decided_by: &str,
    skills_dir: &Path,
) -> Result<ProposalRow, SkillAuthorError> {
    let row = ctx
        .repo
        .get(proposal_id)
        .await?
        .ok_or(SkillAuthorError::NotFound)?;

    // Materialise the YAML BEFORE flipping the status, so a write
    // failure leaves the DB in the original `pending` state.
    let yaml_path = skill_yaml_path(skills_dir, &row.manifest);
    write_skill_yaml(&yaml_path, &row.manifest)?;

    let updated = ctx
        .repo
        .set_status(proposal_id, ProposalStatus::Installed, decided_by, None)
        .await?;

    ctx.audit
        .record(audit_entry(
            decided_by,
            "skill.approve",
            Some(format!(
                "skill:{}@{}",
                row.manifest.name, row.manifest.version
            )),
            serde_json::json!({
                "proposal_id": proposal_id,
                "name": row.manifest.name,
                "version": row.manifest.version,
                "path": yaml_path.display().to_string(),
            }),
        ))
        .await?;

    Ok(updated)
}

/// Reject a pending proposal. No YAML written; audit emitted.
pub async fn reject_proposal(
    ctx: &SkillAuthorCtx<'_>,
    proposal_id: &str,
    decided_by: &str,
    reason: &str,
) -> Result<ProposalRow, SkillAuthorError> {
    let row = ctx
        .repo
        .get(proposal_id)
        .await?
        .ok_or(SkillAuthorError::NotFound)?;

    let updated = ctx
        .repo
        .set_status(
            proposal_id,
            ProposalStatus::Rejected,
            decided_by,
            Some(reason),
        )
        .await?;

    ctx.audit
        .record(audit_entry(
            decided_by,
            "skill.reject",
            Some(format!(
                "skill:{}@{}",
                row.manifest.name, row.manifest.version
            )),
            serde_json::json!({
                "proposal_id": proposal_id,
                "reason": reason,
            }),
        ))
        .await?;

    Ok(updated)
}

fn skill_yaml_path(dir: &Path, m: &SkillManifest) -> PathBuf {
    dir.join(format!("{}-{}.yaml", m.name, m.version))
}

fn write_skill_yaml(path: &Path, m: &SkillManifest) -> Result<(), SkillAuthorError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| SkillAuthorError::Backend(format!("mkdir {}: {e}", parent.display())))?;
    }
    if path.exists() {
        return Err(SkillAuthorError::SkillFileExists);
    }
    let yaml = serde_yaml::to_string(m).map_err(|e| SkillAuthorError::YamlRender(e.to_string()))?;
    std::fs::write(path, yaml)
        .map_err(|e| SkillAuthorError::Backend(format!("write {}: {e}", path.display())))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// In-memory test fixtures
// ---------------------------------------------------------------------------

use parking_lot::Mutex;

/// In-memory `SkillProposalRepository` for unit + integration tests.
#[derive(Debug, Default)]
pub struct InMemorySkillProposalRepository {
    rows: Mutex<Vec<ProposalRow>>,
}

impl InMemorySkillProposalRepository {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

#[async_trait]
impl SkillProposalRepository for InMemorySkillProposalRepository {
    async fn insert(&self, row: ProposalRow) -> Result<ProposalRow, SkillAuthorError> {
        let mut rows = self.rows.lock();
        let clash = rows.iter().any(|r| {
            r.manifest.name == row.manifest.name && r.manifest.version == row.manifest.version
        });
        if clash {
            return Err(SkillAuthorError::Duplicate);
        }
        rows.push(row.clone());
        Ok(row)
    }

    async fn get(&self, id: &str) -> Result<Option<ProposalRow>, SkillAuthorError> {
        Ok(self.rows.lock().iter().find(|r| r.id == id).cloned())
    }

    async fn list(
        &self,
        status: Option<ProposalStatus>,
    ) -> Result<Vec<ProposalRow>, SkillAuthorError> {
        let rows = self.rows.lock();
        let mut out: Vec<ProposalRow> = rows
            .iter()
            .filter(|r| status.is_none_or(|s| r.status == s))
            .cloned()
            .collect();
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(out)
    }

    async fn set_status(
        &self,
        id: &str,
        status: ProposalStatus,
        decided_by: &str,
        reason: Option<&str>,
    ) -> Result<ProposalRow, SkillAuthorError> {
        let mut rows = self.rows.lock();
        for r in rows.iter_mut() {
            if r.id == id {
                r.status = status;
                r.decided_at = Some(Utc::now());
                r.decided_by = Some(decided_by.to_string());
                if let Some(reason) = reason {
                    r.reason = Some(reason.to_string());
                }
                return Ok(r.clone());
            }
        }
        Err(SkillAuthorError::NotFound)
    }
}

/// In-memory `TenantSettingsReader` — returns whatever was set.
#[derive(Debug, Default)]
pub struct InMemoryTenantSettings {
    allowed: Mutex<bool>,
    tier: Mutex<Option<SandboxTier>>,
}

impl InMemoryTenantSettings {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn allow(&self) {
        *self.allowed.lock() = true;
    }

    /// Set the sandbox tier (DEC-019). Default is L1.
    pub fn set_sandbox_tier(&self, tier: SandboxTier) {
        *self.tier.lock() = Some(tier);
    }
}

#[async_trait]
impl TenantSettingsReader for InMemoryTenantSettings {
    async fn allow_skill_authoring(&self) -> Result<bool, SkillAuthorError> {
        Ok(*self.allowed.lock())
    }

    async fn sandbox_tier(&self) -> Result<SandboxTier, SkillAuthorError> {
        Ok(self.tier.lock().unwrap_or(SandboxTier::L1))
    }
}

/// Always-allow gate for tests.
#[derive(Debug, Default, Clone)]
pub struct AllowAllSkillGate;

#[async_trait]
impl SkillAuthorGate for AllowAllSkillGate {
    async fn check(&self, _scope: &str) -> Result<(), String> {
        Ok(())
    }
}

/// Always-deny gate for tests.
#[derive(Debug, Clone)]
pub struct DenySkillGate {
    pub reason: String,
}

impl DenySkillGate {
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

#[async_trait]
impl SkillAuthorGate for DenySkillGate {
    async fn check(&self, _scope: &str) -> Result<(), String> {
        Err(self.reason.clone())
    }
}

/// Stateful gate that denies the first N calls then allows. Used in the
/// E2E test to model the "agent learns from a denial and retries" flow.
#[derive(Debug)]
pub struct DenyThenAllowGate {
    remaining_denies: Mutex<u32>,
    reason: String,
}

impl DenyThenAllowGate {
    #[must_use]
    pub fn new(denies: u32, reason: impl Into<String>) -> Self {
        Self {
            remaining_denies: Mutex::new(denies),
            reason: reason.into(),
        }
    }
}

#[async_trait]
impl SkillAuthorGate for DenyThenAllowGate {
    async fn check(&self, _scope: &str) -> Result<(), String> {
        let mut left = self.remaining_denies.lock();
        if *left > 0 {
            *left -= 1;
            Err(self.reason.clone())
        } else {
            Ok(())
        }
    }
}

/// In-memory audit sink. Tests read `entries()` to assert the chain.
#[derive(Debug, Default)]
pub struct InMemoryAuditSink {
    entries: Mutex<Vec<AuditEntry>>,
}

impl InMemoryAuditSink {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    #[must_use]
    pub fn entries(&self) -> Vec<AuditEntry> {
        self.entries.lock().clone()
    }
}

#[async_trait]
impl SkillAuditSink for InMemoryAuditSink {
    async fn record(&self, entry: AuditEntry) -> Result<(), SkillAuthorError> {
        self.entries.lock().push(entry);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn known_tools() -> HashSet<String> {
        ["search", "fetch_url"]
            .into_iter()
            .map(String::from)
            .collect()
    }

    fn good_manifest() -> SkillManifest {
        SkillManifest {
            name: "ar-collector".into(),
            description: "Collect overdue AR invoices".into(),
            version: "0.1.0".into(),
            system_prompt: "You collect AR".into(),
            tool_allowlist: vec!["search".into(), "fetch_url".into()],
        }
    }

    fn ctx<'a>(
        repo: &'a (dyn SkillProposalRepository + 'a),
        settings: &'a (dyn TenantSettingsReader + 'a),
        gate: &'a (dyn SkillAuthorGate + 'a),
        audit: &'a (dyn SkillAuditSink + 'a),
        known: &'a HashSet<String>,
    ) -> SkillAuthorCtx<'a> {
        SkillAuthorCtx {
            repo,
            settings,
            gate,
            audit,
            known_tools: known,
        }
    }

    // ── validator -----------------------------------------------------------

    #[test]
    fn validate_rejects_empty_name() {
        let mut m = good_manifest();
        m.name.clear();
        let err = validate_manifest(&m, &known_tools()).unwrap_err();
        assert!(matches!(err, SkillAuthorError::InvalidManifest(_)));
    }

    #[test]
    fn validate_rejects_bad_version() {
        let mut m = good_manifest();
        m.version = "v1".into();
        let err = validate_manifest(&m, &known_tools()).unwrap_err();
        assert!(matches!(err, SkillAuthorError::InvalidManifest(_)));
    }

    #[test]
    fn validate_rejects_empty_allowlist() {
        let mut m = good_manifest();
        m.tool_allowlist.clear();
        let err = validate_manifest(&m, &known_tools()).unwrap_err();
        assert!(matches!(err, SkillAuthorError::InvalidManifest(_)));
    }

    #[test]
    fn validate_rejects_unknown_tool() {
        let mut m = good_manifest();
        m.tool_allowlist.push("rm_rf".into());
        let err = validate_manifest(&m, &known_tools()).unwrap_err();
        assert!(matches!(err, SkillAuthorError::InvalidManifest(_)));
    }

    #[test]
    fn validate_rejects_propose_skill_recursion() {
        let mut m = good_manifest();
        m.tool_allowlist.push(PROPOSE_SKILL_TOOL_NAME.into());
        let err = validate_manifest(&m, &known_tools()).unwrap_err();
        assert!(matches!(err, SkillAuthorError::InvalidManifest(s) if s.contains("propose_skill")));
    }

    #[test]
    fn validate_rejects_bad_name_chars() {
        let mut m = good_manifest();
        m.name = "ar collector".into();
        let err = validate_manifest(&m, &known_tools()).unwrap_err();
        assert!(matches!(err, SkillAuthorError::InvalidManifest(_)));
    }

    #[test]
    fn validate_accepts_good_manifest() {
        validate_manifest(&good_manifest(), &known_tools()).unwrap();
    }

    // ── propose -------------------------------------------------------------

    #[tokio::test]
    async fn propose_disabled_returns_disabled() {
        let repo = InMemorySkillProposalRepository::new();
        let settings = InMemoryTenantSettings::new(); // NOT allowed
        let gate = AllowAllSkillGate;
        let audit = InMemoryAuditSink::new();
        let known = known_tools();
        let ctx = ctx(&*repo, &*settings, &gate, &*audit, &known);

        let err = propose(&ctx, "agent-1", good_manifest()).await.unwrap_err();
        assert!(matches!(err, SkillAuthorError::Disabled));
        // Off-by-default is also off-the-record — no audit row.
        assert!(audit.entries().is_empty());
    }

    #[tokio::test]
    async fn propose_with_unknown_tool_rejected_before_gate() {
        let repo = InMemorySkillProposalRepository::new();
        let settings = InMemoryTenantSettings::new();
        settings.allow();
        let gate = DenySkillGate::new("would-have-denied");
        let audit = InMemoryAuditSink::new();
        let known = known_tools();
        let ctx = ctx(&*repo, &*settings, &gate, &*audit, &known);

        let mut m = good_manifest();
        m.tool_allowlist.push("rm_rf".into());
        let err = propose(&ctx, "agent-1", m).await.unwrap_err();
        assert!(matches!(err, SkillAuthorError::InvalidManifest(_)));
        // Validation precedes gate — no audit rows, no gate consultation.
        assert!(audit.entries().is_empty());
    }

    #[tokio::test]
    async fn propose_with_denied_gate_returns_denied_and_records_two_audit_rows() {
        let repo = InMemorySkillProposalRepository::new();
        let settings = InMemoryTenantSettings::new();
        settings.allow();
        let gate = DenySkillGate::new("budget exceeded");
        let audit = InMemoryAuditSink::new();
        let known = known_tools();
        let ctx = ctx(&*repo, &*settings, &gate, &*audit, &known);

        let err = propose(&ctx, "agent-1", good_manifest()).await.unwrap_err();
        match err {
            SkillAuthorError::Denied(r) => assert_eq!(r, "budget exceeded"),
            other => panic!("expected Denied, got {other:?}"),
        }
        let entries = audit.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].action, "skill.propose");
        assert_eq!(entries[1].action, "skill.hotl_gate");
        // Storage was NOT touched — gate denial drops the proposal.
        assert!(repo.list(None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn propose_with_allowed_gate_persists_pending_row() {
        let repo = InMemorySkillProposalRepository::new();
        let settings = InMemoryTenantSettings::new();
        settings.allow();
        let gate = AllowAllSkillGate;
        let audit = InMemoryAuditSink::new();
        let known = known_tools();
        let ctx = ctx(&*repo, &*settings, &gate, &*audit, &known);

        let row = propose(&ctx, "agent-1", good_manifest()).await.unwrap();
        assert_eq!(row.status, ProposalStatus::Pending);
        assert_eq!(audit.entries().len(), 2);
        assert_eq!(
            repo.list(Some(ProposalStatus::Pending))
                .await
                .unwrap()
                .len(),
            1
        );
    }

    // ── approve / reject ----------------------------------------------------

    #[tokio::test]
    async fn approve_writes_yaml_and_flips_status() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = InMemorySkillProposalRepository::new();
        let settings = InMemoryTenantSettings::new();
        settings.allow();
        let gate = AllowAllSkillGate;
        let audit = InMemoryAuditSink::new();
        let known = known_tools();
        let ctx = ctx(&*repo, &*settings, &gate, &*audit, &known);

        let row = propose(&ctx, "agent-1", good_manifest()).await.unwrap();
        let updated = approve_proposal(&ctx, &row.id, "admin-1", tmp.path())
            .await
            .unwrap();
        assert_eq!(updated.status, ProposalStatus::Installed);
        assert_eq!(updated.decided_by.as_deref(), Some("admin-1"));

        let file = tmp.path().join("ar-collector-0.1.0.yaml");
        assert!(file.exists(), "yaml should land on disk");
        let parsed: SkillManifest =
            serde_yaml::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(parsed, good_manifest());

        // Three audit rows in total: propose, gate, approve.
        let actions: Vec<_> = audit.entries().iter().map(|e| e.action.clone()).collect();
        assert_eq!(
            actions,
            vec!["skill.propose", "skill.hotl_gate", "skill.approve"]
        );
    }

    #[tokio::test]
    async fn approve_missing_proposal_returns_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = InMemorySkillProposalRepository::new();
        let settings = InMemoryTenantSettings::new();
        let gate = AllowAllSkillGate;
        let audit = InMemoryAuditSink::new();
        let known = known_tools();
        let ctx = ctx(&*repo, &*settings, &gate, &*audit, &known);

        let err = approve_proposal(&ctx, "no-such-id", "admin-1", tmp.path())
            .await
            .unwrap_err();
        assert!(matches!(err, SkillAuthorError::NotFound));
    }

    #[tokio::test]
    async fn approve_fails_if_yaml_already_exists_and_does_not_flip_status() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = InMemorySkillProposalRepository::new();
        let settings = InMemoryTenantSettings::new();
        settings.allow();
        let gate = AllowAllSkillGate;
        let audit = InMemoryAuditSink::new();
        let known = known_tools();
        let ctx = ctx(&*repo, &*settings, &gate, &*audit, &known);

        // Pre-create the YAML at the target path.
        std::fs::write(tmp.path().join("ar-collector-0.1.0.yaml"), "stale: true").unwrap();

        let row = propose(&ctx, "agent-1", good_manifest()).await.unwrap();
        let err = approve_proposal(&ctx, &row.id, "admin-1", tmp.path())
            .await
            .unwrap_err();
        assert!(matches!(err, SkillAuthorError::SkillFileExists));

        // DB row remains pending (no orphaned `installed` row).
        let still = repo.get(&row.id).await.unwrap().unwrap();
        assert_eq!(still.status, ProposalStatus::Pending);
    }

    #[tokio::test]
    async fn reject_flips_status_and_records_audit() {
        let tmp = tempfile::tempdir().unwrap();
        let _ = tmp;
        let repo = InMemorySkillProposalRepository::new();
        let settings = InMemoryTenantSettings::new();
        settings.allow();
        let gate = AllowAllSkillGate;
        let audit = InMemoryAuditSink::new();
        let known = known_tools();
        let ctx = ctx(&*repo, &*settings, &gate, &*audit, &known);

        let row = propose(&ctx, "agent-1", good_manifest()).await.unwrap();
        let updated = reject_proposal(&ctx, &row.id, "admin-1", "too broad")
            .await
            .unwrap();
        assert_eq!(updated.status, ProposalStatus::Rejected);
        assert_eq!(updated.reason.as_deref(), Some("too broad"));

        let actions: Vec<_> = audit.entries().iter().map(|e| e.action.clone()).collect();
        assert_eq!(
            actions,
            vec!["skill.propose", "skill.hotl_gate", "skill.reject"]
        );
    }

    // ── Repository semantics ------------------------------------------------

    #[tokio::test]
    async fn duplicate_proposal_rejected() {
        let repo = InMemorySkillProposalRepository::new();
        let mk = |id: &str| ProposalRow {
            id: id.into(),
            proposed_by: "agent-1".into(),
            manifest: good_manifest(),
            status: ProposalStatus::Pending,
            reason: None,
            created_at: Utc::now(),
            decided_at: None,
            decided_by: None,
        };
        repo.insert(mk("a")).await.unwrap();
        let err = repo.insert(mk("b")).await.unwrap_err();
        assert!(matches!(err, SkillAuthorError::Duplicate));
    }

    #[tokio::test]
    async fn list_filters_by_status_and_orders_newest_first() {
        let repo = InMemorySkillProposalRepository::new();
        let settings = InMemoryTenantSettings::new();
        settings.allow();
        let gate = AllowAllSkillGate;
        let audit = InMemoryAuditSink::new();
        let known = known_tools();
        let ctx = ctx(&*repo, &*settings, &gate, &*audit, &known);

        let m1 = SkillManifest {
            version: "0.1.0".into(),
            ..good_manifest()
        };
        let m2 = SkillManifest {
            version: "0.2.0".into(),
            ..good_manifest()
        };
        let r1 = propose(&ctx, "agent-1", m1).await.unwrap();
        // Force a measurable gap so created_at sort is stable.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let r2 = propose(&ctx, "agent-1", m2).await.unwrap();

        let all = repo.list(None).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, r2.id, "newest first");
        assert_eq!(all[1].id, r1.id);
    }

    // ── ProposalStatus ------------------------------------------------------

    #[test]
    fn proposal_status_round_trips() {
        for s in [
            ProposalStatus::Pending,
            ProposalStatus::Approved,
            ProposalStatus::Rejected,
            ProposalStatus::Installed,
        ] {
            assert_eq!(ProposalStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(ProposalStatus::parse("nope"), None);
    }

    // ── SandboxTier (DEC-019) -----------------------------------------------

    #[test]
    fn sandbox_tier_parse_lenient_l3() {
        assert_eq!(SandboxTier::from_str_lenient("L3"), SandboxTier::L3);
        assert_eq!(SandboxTier::from_str_lenient("l3"), SandboxTier::L3);
        assert_eq!(SandboxTier::from_str_lenient("  L3  "), SandboxTier::L3);
    }

    #[test]
    fn sandbox_tier_unknown_falls_back_to_l1() {
        // Safe-default per PHILO §14 — unknown / malformed → L1.
        assert_eq!(SandboxTier::from_str_lenient("L1"), SandboxTier::L1);
        assert_eq!(SandboxTier::from_str_lenient("l1"), SandboxTier::L1);
        assert_eq!(SandboxTier::from_str_lenient(""), SandboxTier::L1);
        assert_eq!(SandboxTier::from_str_lenient("L2"), SandboxTier::L1);
        assert_eq!(SandboxTier::from_str_lenient("L4"), SandboxTier::L1);
        assert_eq!(SandboxTier::from_str_lenient("garbage"), SandboxTier::L1);
    }

    #[test]
    fn sandbox_tier_labels_are_stable() {
        // Stable labels are load-bearing for metrics and dashboards;
        // changing them silently breaks operator alerting.
        assert_eq!(SandboxTier::L1.as_str(), "L1");
        assert_eq!(SandboxTier::L3.as_str(), "L3");
    }

    #[tokio::test]
    async fn in_memory_tenant_settings_sandbox_tier_defaults_to_l1() {
        let s = InMemoryTenantSettings::new();
        // Untouched → L1 default.
        assert_eq!(s.sandbox_tier().await.unwrap(), SandboxTier::L1);
    }

    #[tokio::test]
    async fn in_memory_tenant_settings_sandbox_tier_round_trip() {
        let s = InMemoryTenantSettings::new();
        s.set_sandbox_tier(SandboxTier::L3);
        assert_eq!(s.sandbox_tier().await.unwrap(), SandboxTier::L3);
    }
}
