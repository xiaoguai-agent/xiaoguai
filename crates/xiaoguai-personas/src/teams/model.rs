//! Domain types for expert teams (T3 — expert center).
//!
//! A *team* is a named composition of personas: an ordered member list with a
//! designated lead. Until T4 (parallel orchestration) lands, a team session
//! runs with the **lead persona only** — attaching a team to a session also
//! attaches its lead via the existing `session_personas` path, so the ReAct
//! loop is unchanged. Teams are composition objects, NOT access-control
//! objects (DEC-033 single owner).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{PersonaError, PersonaResult};

/// A named composition of personas with a designated lead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub id: Uuid,
    /// Human-readable label. Unique by name (enforced at DB level).
    pub name: String,
    pub description: String,
    /// Runs the session until T4 parallel orchestration. Must be a member.
    pub lead_persona_id: Uuid,
    /// Ordered, deduplicated, non-empty; includes the lead.
    pub member_persona_ids: Vec<Uuid>,
    /// Display-only pack suggestions shown at selection time (owner decision
    /// ③A: tags only — installation stays in admin-ui).
    pub recommended_pack_slugs: Vec<String>,
    pub created_at: DateTime<Utc>,
    /// Soft-deleted teams cannot be attached to new sessions.
    pub archived: bool,
}

/// Payload used when creating a new team.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTeamRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub lead_persona_id: Uuid,
    pub member_persona_ids: Vec<Uuid>,
    #[serde(default)]
    pub recommended_pack_slugs: Vec<String>,
}

/// Payload used when updating an existing team.
///
/// Only non-`None` fields are applied; composition rules are re-validated
/// against the *merged* result (e.g. a new lead must be in the effective
/// member list).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateTeamRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub lead_persona_id: Option<Uuid>,
    pub member_persona_ids: Option<Vec<Uuid>>,
    pub recommended_pack_slugs: Option<Vec<String>>,
}

/// Records which team is attached to a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTeam {
    pub session_id: String,
    pub team_id: Uuid,
    pub attached_at: DateTime<Utc>,
}

/// Validate the structural composition rules shared by create and update.
///
/// Rules: at least one member, no duplicate members, lead must be a member.
/// Persona *existence/active* checks are the API boundary's job (it holds the
/// `PersonaRepository`); this is pure structure.
pub fn validate_composition(lead: Uuid, members: &[Uuid]) -> PersonaResult<()> {
    if members.is_empty() {
        return Err(PersonaError::InvalidArgument(
            "a team needs at least one member persona".to_string(),
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for m in members {
        if !seen.insert(m) {
            return Err(PersonaError::InvalidArgument(format!(
                "duplicate member persona: {m}"
            )));
        }
    }
    if !members.contains(&lead) {
        return Err(PersonaError::InvalidArgument(
            "the lead persona must be one of the team members".to_string(),
        ));
    }
    Ok(())
}
