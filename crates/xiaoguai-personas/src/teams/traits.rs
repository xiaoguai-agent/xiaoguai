//! `TeamRepository` trait — the only interface business code touches.
//!
//! Mirrors `PersonaRepository`. Implementations enforce the *structural*
//! composition rules ([`crate::teams::model::validate_composition`]); checking
//! that member personas exist and are active is the API boundary's job, since
//! it holds the `PersonaRepository`.

use async_trait::async_trait;
use uuid::Uuid;

use crate::error::PersonaResult;
use crate::teams::model::{CreateTeamRequest, SessionTeam, Team, UpdateTeamRequest};

#[async_trait]
pub trait TeamRepository: Send + Sync {
    // ── Team CRUD ─────────────────────────────────────────────────────────────

    /// List all non-archived teams, ordered by name.
    async fn list(&self) -> PersonaResult<Vec<Team>>;

    /// Fetch a single team by its UUID. Returns `NotFound` if absent.
    async fn get(&self, id: Uuid) -> PersonaResult<Team>;

    /// Insert a new team. Returns `DuplicateName` if `name` already exists and
    /// `InvalidArgument` if the composition rules are violated.
    async fn create(&self, req: &CreateTeamRequest) -> PersonaResult<Team>;

    /// Apply non-`None` fields from `req`, re-validating composition against
    /// the merged result. Returns the updated team; `NotFound` if absent.
    async fn update(&self, id: Uuid, req: &UpdateTeamRequest) -> PersonaResult<Team>;

    /// Soft-delete a team. Existing session attachments are retained but the
    /// team cannot be attached to new sessions. Idempotent.
    async fn archive_team(&self, id: Uuid) -> PersonaResult<()>;

    // ── Session attachment ────────────────────────────────────────────────────

    /// Attach `team_id` to `session_id`, replacing any previous attachment.
    /// Returns `Archived` if the team has been soft-deleted. NOTE: attaching
    /// the team's lead persona to the session (via `PersonaRepository`) is the
    /// caller's responsibility — the API route does both, persona first.
    async fn attach_team_to_session(
        &self,
        session_id: &str,
        team_id: Uuid,
    ) -> PersonaResult<SessionTeam>;

    /// Detach any team currently attached to `session_id`. Idempotent.
    async fn detach_team_from_session(&self, session_id: &str) -> PersonaResult<()>;

    /// Return the team (if any) currently attached to `session_id`.
    async fn get_session_team(&self, session_id: &str) -> PersonaResult<Option<Team>>;
}
