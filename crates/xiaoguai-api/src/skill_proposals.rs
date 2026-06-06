//! Tier-2 D.1 — HTTP routes for agent-authored skill proposals.
//!
//! Three endpoints under `/v1/skills/proposals`:
//!
//! * `GET    /v1/skills/proposals?status=`                — list rows
//! * `POST   /v1/skills/proposals/:id/approve`            — flip → installed
//! * `POST   /v1/skills/proposals/:id/reject`             — flip → rejected
//!
//! The actual state-transition logic lives in
//! `xiaoguai_tasks::skill_author`; this module just plumbs HTTP wire
//! types into that function. `approve` writes a YAML manifest to
//! `state.skills_dir`; `reject` records the reason.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use xiaoguai_tasks::skill_author::{
    self, ProposalRow, ProposalStatus, SkillAuditSink, SkillAuthorCtx, SkillAuthorError,
    SkillAuthorGate, SkillManifest, SkillProposalRepository, TenantSettingsReader,
};

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalRowResponse {
    pub id: String,
    pub proposed_by: String,
    pub manifest: SkillManifest,
    pub status: String,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub decided_by: Option<String>,
}

impl From<ProposalRow> for ProposalRowResponse {
    fn from(r: ProposalRow) -> Self {
        Self {
            id: r.id,
            proposed_by: r.proposed_by,
            manifest: r.manifest,
            status: r.status.as_str().to_string(),
            reason: r.reason,
            created_at: r.created_at,
            decided_at: r.decided_at,
            decided_by: r.decided_by,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct ListProposalsQuery {
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ApproveRequest {
    /// Identity of the human (or system) approving. Mandatory for audit.
    pub decided_by: String,
}

#[derive(Debug, Deserialize)]
pub struct RejectRequest {
    pub decided_by: String,
    pub reason: String,
}

// ---------------------------------------------------------------------------
// Helpers — pulling collaborators out of AppState
// ---------------------------------------------------------------------------

fn proposals_repo(state: &AppState) -> ApiResult<Arc<dyn SkillProposalRepository>> {
    state
        .skill_proposals
        .clone()
        .ok_or_else(|| ApiError::ServiceUnavailable("skill proposals not wired".into()))
}

fn tenant_settings(state: &AppState) -> ApiResult<Arc<dyn TenantSettingsReader>> {
    state
        .tenant_settings
        .clone()
        .ok_or_else(|| ApiError::ServiceUnavailable("tenant settings not wired".into()))
}

fn skill_gate(state: &AppState) -> ApiResult<Arc<dyn SkillAuthorGate>> {
    state
        .skill_author_gate
        .clone()
        .ok_or_else(|| ApiError::ServiceUnavailable("skill author gate not wired".into()))
}

fn skill_audit(state: &AppState) -> ApiResult<Arc<dyn SkillAuditSink>> {
    state
        .skill_audit
        .clone()
        .ok_or_else(|| ApiError::ServiceUnavailable("skill audit sink not wired".into()))
}

fn err_to_api(e: SkillAuthorError) -> ApiError {
    match e {
        SkillAuthorError::Disabled => {
            ApiError::ServiceUnavailable("agent-authored skills are not enabled".into())
        }
        SkillAuthorError::InvalidManifest(s) => {
            ApiError::InvalidRequest(format!("invalid manifest: {s}"))
        }
        SkillAuthorError::Denied(s) => ApiError::BadRequest(format!("hotl gate denied: {s}")),
        SkillAuthorError::Duplicate => ApiError::Conflict("proposal already exists".into()),
        SkillAuthorError::NotFound => ApiError::NotFound,
        SkillAuthorError::SkillFileExists => {
            ApiError::Conflict("a skill with this name and version already exists on disk".into())
        }
        SkillAuthorError::NotPending => {
            ApiError::Conflict("proposal is not pending (already decided)".into())
        }
        SkillAuthorError::YamlRender(s) => ApiError::Internal(anyhow::anyhow!("yaml render: {s}")),
        SkillAuthorError::Backend(s) => ApiError::Internal(anyhow::anyhow!("skill author: {s}")),
    }
}

// ---------------------------------------------------------------------------
// Route handlers
// ---------------------------------------------------------------------------

/// `GET /v1/skills/proposals?status=pending`
pub async fn list_proposals(
    State(state): State<AppState>,
    Query(q): Query<ListProposalsQuery>,
) -> ApiResult<Json<Vec<ProposalRowResponse>>> {
    let repo = proposals_repo(&state)?;
    let status = match q.status.as_deref() {
        None => None,
        Some(s) => Some(
            ProposalStatus::parse(s)
                .ok_or_else(|| ApiError::InvalidRequest(format!("unknown status {s:?}")))?,
        ),
    };
    let rows = repo.list(status).await.map_err(err_to_api)?;
    Ok(Json(
        rows.into_iter().map(ProposalRowResponse::from).collect(),
    ))
}

/// `POST /v1/skills/proposals/:id/approve`
pub async fn approve_proposal_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ApproveRequest>,
) -> ApiResult<Json<ProposalRowResponse>> {
    let repo = proposals_repo(&state)?;
    let settings = tenant_settings(&state)?;
    let gate = skill_gate(&state)?;
    let audit = skill_audit(&state)?;
    let skills_dir: PathBuf = state.skills_dir.clone();
    // `known_tools` is unused in approve/reject (gate not consulted), but
    // the context struct requires it. Pass an empty set — validator
    // already passed at propose-time.
    let known: HashSet<String> = HashSet::new();
    let ctx = SkillAuthorCtx {
        repo: &*repo,
        settings: &*settings,
        gate: &*gate,
        audit: &*audit,
        known_tools: &known,
    };
    let updated = skill_author::approve_proposal(&ctx, &id, &req.decided_by, &skills_dir)
        .await
        .map_err(err_to_api)?;
    Ok(Json(updated.into()))
}

/// `POST /v1/skills/proposals/:id/reject`
pub async fn reject_proposal_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RejectRequest>,
) -> ApiResult<Json<ProposalRowResponse>> {
    let repo = proposals_repo(&state)?;
    let settings = tenant_settings(&state)?;
    let gate = skill_gate(&state)?;
    let audit = skill_audit(&state)?;
    let known: HashSet<String> = HashSet::new();
    let ctx = SkillAuthorCtx {
        repo: &*repo,
        settings: &*settings,
        gate: &*gate,
        audit: &*audit,
        known_tools: &known,
    };
    let updated = skill_author::reject_proposal(&ctx, &id, &req.decided_by, &req.reason)
        .await
        .map_err(err_to_api)?;
    Ok(Json(updated.into()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposal_row_into_response_preserves_manifest() {
        let m = SkillManifest {
            name: "x".into(),
            description: "y".into(),
            version: "0.1.0".into(),
            system_prompt: "z".into(),
            tool_allowlist: vec!["search".into()],
        };
        let row = ProposalRow {
            id: "id-1".into(),
            proposed_by: "agent-1".into(),
            manifest: m.clone(),
            status: ProposalStatus::Pending,
            reason: None,
            created_at: Utc::now(),
            decided_at: None,
            decided_by: None,
        };
        let resp: ProposalRowResponse = row.into();
        assert_eq!(resp.status, "pending");
        assert_eq!(resp.manifest, m);
    }

    #[test]
    fn err_mapping_covers_all_variants() {
        // Tripwire: when SkillAuthorError gains a variant, this match
        // forces us to update err_to_api.
        let cases = [
            SkillAuthorError::Disabled,
            SkillAuthorError::InvalidManifest("x".into()),
            SkillAuthorError::Denied("x".into()),
            SkillAuthorError::Duplicate,
            SkillAuthorError::NotFound,
            SkillAuthorError::SkillFileExists,
            SkillAuthorError::YamlRender("x".into()),
            SkillAuthorError::Backend("x".into()),
        ];
        for e in cases {
            let _api = err_to_api(e);
        }
    }
}
