//! `SQLite`-backed `TeamRepository` implementation.
//!
//! Mirrors `SqlitePersonaRepository` (DEC-033 single owner). `expert_teams`
//! stores `member_persona_ids` / `recommended_pack_slugs` as TEXT holding a
//! JSON array (member ids therefore have no per-row FK — the API boundary
//! verifies members against the `PersonaRepository`; the lead has a real FK).
//! `session_teams` has `session_id` as PRIMARY KEY, enforcing one team per
//! session at the DB level via `ON CONFLICT DO UPDATE`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

use crate::error::{PersonaError, PersonaResult};
use crate::teams::model::{
    validate_composition, CreateTeamRequest, SessionTeam, Team, UpdateTeamRequest,
};
use crate::teams::traits::TeamRepository;

/// `SQLite` implementation. Clone is cheap — `SqlitePool` is an `Arc` internally.
#[derive(Debug, Clone)]
pub struct SqliteTeamRepository {
    pool: SqlitePool,
}

impl SqliteTeamRepository {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

// ── JSON-array <-> Vec helpers (TEXT[] replacement) ───────────────────────────

fn uuids_to_text(ids: &[Uuid]) -> String {
    serde_json::to_string(&ids.iter().map(Uuid::to_string).collect::<Vec<_>>())
        .unwrap_or_else(|_| "[]".to_string())
}

/// Malformed text degrades to an empty list rather than failing the read.
fn text_to_uuids(text: &str) -> Vec<Uuid> {
    serde_json::from_str::<Vec<String>>(text)
        .unwrap_or_default()
        .iter()
        .filter_map(|s| Uuid::parse_str(s).ok())
        .collect()
}

fn slugs_to_text(slugs: &[String]) -> String {
    serde_json::to_string(slugs).unwrap_or_else(|_| "[]".to_string())
}

fn text_to_slugs(text: Option<String>) -> Vec<String> {
    text.map(|t| serde_json::from_str::<Vec<String>>(&t).unwrap_or_default())
        .unwrap_or_default()
}

// ── Row types (sqlx deserialization) ─────────────────────────────────────────

const TEAM_COLS: &str = "id, name, description, lead_persona_id, \
                         member_persona_ids, recommended_pack_slugs, created_at, archived";

#[derive(Debug, FromRow)]
struct TeamRow {
    id: String,
    name: String,
    description: String,
    lead_persona_id: String,
    member_persona_ids: String,
    recommended_pack_slugs: Option<String>,
    created_at: DateTime<Utc>,
    archived: bool,
}

impl From<TeamRow> for Team {
    fn from(r: TeamRow) -> Self {
        Self {
            id: Uuid::parse_str(&r.id).unwrap_or_else(|_| Uuid::nil()),
            name: r.name,
            description: r.description,
            lead_persona_id: Uuid::parse_str(&r.lead_persona_id).unwrap_or_else(|_| Uuid::nil()),
            member_persona_ids: text_to_uuids(&r.member_persona_ids),
            recommended_pack_slugs: text_to_slugs(r.recommended_pack_slugs),
            created_at: r.created_at,
            archived: r.archived,
        }
    }
}

#[derive(Debug, FromRow)]
struct SessionTeamRow {
    session_id: String,
    team_id: String,
    attached_at: DateTime<Utc>,
}

impl From<SessionTeamRow> for SessionTeam {
    fn from(r: SessionTeamRow) -> Self {
        Self {
            session_id: r.session_id,
            team_id: Uuid::parse_str(&r.team_id).unwrap_or_else(|_| Uuid::nil()),
            attached_at: r.attached_at,
        }
    }
}

// ── Trait implementation ──────────────────────────────────────────────────────

#[async_trait]
impl TeamRepository for SqliteTeamRepository {
    async fn list(&self) -> PersonaResult<Vec<Team>> {
        let rows: Vec<TeamRow> = sqlx::query_as(&format!(
            "SELECT {TEAM_COLS} FROM expert_teams WHERE NOT archived ORDER BY name ASC"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(PersonaError::from_sqlx)?;
        Ok(rows.into_iter().map(Team::from).collect())
    }

    async fn get(&self, id: Uuid) -> PersonaResult<Team> {
        let row: Option<TeamRow> = sqlx::query_as(&format!(
            "SELECT {TEAM_COLS} FROM expert_teams WHERE id = ?"
        ))
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(PersonaError::from_sqlx)?;
        row.map(Team::from).ok_or(PersonaError::NotFound)
    }

    async fn create(&self, req: &CreateTeamRequest) -> PersonaResult<Team> {
        validate_composition(req.lead_persona_id, &req.member_persona_ids)?;
        let id = Uuid::new_v4();
        let now = Utc::now();
        let row: TeamRow = sqlx::query_as(&format!(
            "INSERT INTO expert_teams \
               (id, name, description, lead_persona_id, \
                member_persona_ids, recommended_pack_slugs, created_at, archived) \
             VALUES (?, ?, ?, ?, ?, ?, ?, false) \
             RETURNING {TEAM_COLS}"
        ))
        .bind(id.to_string())
        .bind(&req.name)
        .bind(&req.description)
        .bind(req.lead_persona_id.to_string())
        .bind(uuids_to_text(&req.member_persona_ids))
        .bind(slugs_to_text(&req.recommended_pack_slugs))
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(PersonaError::from_sqlx)?;
        Ok(Team::from(row))
    }

    async fn update(&self, id: Uuid, req: &UpdateTeamRequest) -> PersonaResult<Team> {
        // Fetch current state, merge, validate, then write — same shape as
        // the persona update path.
        let current = self.get(id).await?;

        let new_name = req.name.as_deref().unwrap_or(&current.name);
        let new_description = req.description.as_deref().unwrap_or(&current.description);
        let new_lead = req.lead_persona_id.unwrap_or(current.lead_persona_id);
        let new_members = req
            .member_persona_ids
            .as_ref()
            .unwrap_or(&current.member_persona_ids);
        let new_slugs = req
            .recommended_pack_slugs
            .as_ref()
            .unwrap_or(&current.recommended_pack_slugs);
        validate_composition(new_lead, new_members)?;

        let row: TeamRow = sqlx::query_as(&format!(
            "UPDATE expert_teams \
             SET name = ?, description = ?, lead_persona_id = ?, \
                 member_persona_ids = ?, recommended_pack_slugs = ? \
             WHERE id = ? \
             RETURNING {TEAM_COLS}"
        ))
        .bind(new_name)
        .bind(new_description)
        .bind(new_lead.to_string())
        .bind(uuids_to_text(new_members))
        .bind(slugs_to_text(new_slugs))
        .bind(id.to_string())
        .fetch_one(&self.pool)
        .await
        .map_err(PersonaError::from_sqlx)?;
        Ok(Team::from(row))
    }

    async fn archive_team(&self, id: Uuid) -> PersonaResult<()> {
        sqlx::query("UPDATE expert_teams SET archived = true WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .map_err(PersonaError::from_sqlx)?;
        // Idempotent — no error if already archived or row doesn't exist.
        Ok(())
    }

    async fn attach_team_to_session(
        &self,
        session_id: &str,
        team_id: Uuid,
    ) -> PersonaResult<SessionTeam> {
        // Guard: refuse to attach archived teams.
        let team = self.get(team_id).await?;
        if team.archived {
            return Err(PersonaError::Archived);
        }

        let now = Utc::now();
        let row: SessionTeamRow = sqlx::query_as(
            "INSERT INTO session_teams (session_id, team_id, attached_at) \
             VALUES (?, ?, ?) \
             ON CONFLICT (session_id) \
             DO UPDATE SET team_id = EXCLUDED.team_id, \
                           attached_at = EXCLUDED.attached_at \
             RETURNING session_id, team_id, attached_at",
        )
        .bind(session_id)
        .bind(team_id.to_string())
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(PersonaError::from_sqlx)?;
        Ok(SessionTeam::from(row))
    }

    async fn detach_team_from_session(&self, session_id: &str) -> PersonaResult<()> {
        sqlx::query("DELETE FROM session_teams WHERE session_id = ?")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(PersonaError::from_sqlx)?;
        Ok(())
    }

    async fn get_session_team(&self, session_id: &str) -> PersonaResult<Option<Team>> {
        let row: Option<TeamRow> = sqlx::query_as(
            "SELECT t.id, t.name, t.description, t.lead_persona_id, \
                    t.member_persona_ids, t.recommended_pack_slugs, t.created_at, t.archived \
             FROM session_teams st \
             JOIN expert_teams t ON t.id = st.team_id \
             WHERE st.session_id = ?",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(PersonaError::from_sqlx)?;
        Ok(row.map(Team::from))
    }
}
