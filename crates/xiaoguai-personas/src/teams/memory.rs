//! In-memory `TeamRepository` for unit tests and integration harnesses.
//!
//! Not intended for production use. Mirrors `InMemoryPersonaRepository`:
//! state behind a `parking_lot::Mutex` so the type is `Send + Sync`.

use async_trait::async_trait;
use chrono::Utc;
use parking_lot::Mutex;
use std::collections::HashMap;
use uuid::Uuid;

use crate::error::{PersonaError, PersonaResult};
use crate::teams::model::{
    validate_composition, CreateTeamRequest, SessionTeam, Team, UpdateTeamRequest,
};
use crate::teams::traits::TeamRepository;

#[derive(Default)]
struct Inner {
    teams: HashMap<Uuid, Team>,
    /// `session_id` → `team_id`
    attachments: HashMap<String, Uuid>,
}

/// Thread-safe in-memory store. All operations are synchronous under the mutex.
#[derive(Default)]
pub struct InMemoryTeamRepository {
    state: Mutex<Inner>,
}

impl InMemoryTeamRepository {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl TeamRepository for InMemoryTeamRepository {
    async fn list(&self) -> PersonaResult<Vec<Team>> {
        let g = self.state.lock();
        let mut out: Vec<Team> = g.teams.values().filter(|t| !t.archived).cloned().collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn get(&self, id: Uuid) -> PersonaResult<Team> {
        let g = self.state.lock();
        g.teams.get(&id).cloned().ok_or(PersonaError::NotFound)
    }

    async fn create(&self, req: &CreateTeamRequest) -> PersonaResult<Team> {
        validate_composition(req.lead_persona_id, &req.member_persona_ids)?;
        let mut g = self.state.lock();
        let duplicate = g.teams.values().any(|t| t.name == req.name && !t.archived);
        if duplicate {
            return Err(PersonaError::DuplicateName(req.name.clone()));
        }
        let team = Team {
            id: Uuid::new_v4(),
            name: req.name.clone(),
            description: req.description.clone(),
            lead_persona_id: req.lead_persona_id,
            member_persona_ids: req.member_persona_ids.clone(),
            recommended_pack_slugs: req.recommended_pack_slugs.clone(),
            created_at: Utc::now(),
            archived: false,
        };
        g.teams.insert(team.id, team.clone());
        Ok(team)
    }

    async fn update(&self, id: Uuid, req: &UpdateTeamRequest) -> PersonaResult<Team> {
        let mut g = self.state.lock();
        let current = g.teams.get(&id).ok_or(PersonaError::NotFound)?;

        // Build the merged result first; only commit if it validates.
        let merged = Team {
            id: current.id,
            name: req.name.clone().unwrap_or_else(|| current.name.clone()),
            description: req
                .description
                .clone()
                .unwrap_or_else(|| current.description.clone()),
            lead_persona_id: req.lead_persona_id.unwrap_or(current.lead_persona_id),
            member_persona_ids: req
                .member_persona_ids
                .clone()
                .unwrap_or_else(|| current.member_persona_ids.clone()),
            recommended_pack_slugs: req
                .recommended_pack_slugs
                .clone()
                .unwrap_or_else(|| current.recommended_pack_slugs.clone()),
            created_at: current.created_at,
            archived: current.archived,
        };
        validate_composition(merged.lead_persona_id, &merged.member_persona_ids)?;
        g.teams.insert(id, merged.clone());
        Ok(merged)
    }

    async fn archive_team(&self, id: Uuid) -> PersonaResult<()> {
        let mut g = self.state.lock();
        if let Some(t) = g.teams.get_mut(&id) {
            t.archived = true;
        }
        Ok(())
    }

    async fn attach_team_to_session(
        &self,
        session_id: &str,
        team_id: Uuid,
    ) -> PersonaResult<SessionTeam> {
        let mut g = self.state.lock();
        let team = g.teams.get(&team_id).ok_or(PersonaError::NotFound)?;
        if team.archived {
            return Err(PersonaError::Archived);
        }
        let now = Utc::now();
        g.attachments.insert(session_id.to_string(), team_id);
        Ok(SessionTeam {
            session_id: session_id.to_string(),
            team_id,
            attached_at: now,
        })
    }

    async fn detach_team_from_session(&self, session_id: &str) -> PersonaResult<()> {
        self.state.lock().attachments.remove(session_id);
        Ok(())
    }

    async fn get_session_team(&self, session_id: &str) -> PersonaResult<Option<Team>> {
        let g = self.state.lock();
        let team_id = match g.attachments.get(session_id) {
            Some(id) => *id,
            None => return Ok(None),
        };
        Ok(g.teams.get(&team_id).cloned())
    }
}
