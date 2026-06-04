//! `PersonaRepository` trait — the only interface business code touches.
//!
//! Implementations may be `SQLite`-backed (`SqlitePersonaRepository`), an
//! in-memory store for tests, or any future alternative. Storage details
//! never leak past this boundary.

use async_trait::async_trait;
use uuid::Uuid;

use crate::error::PersonaResult;
use crate::model::{CreatePersonaRequest, Persona, SessionPersona, UpdatePersonaRequest};

#[async_trait]
pub trait PersonaRepository: Send + Sync {
    // ── Persona CRUD ─────────────────────────────────────────────────────────

    /// List all non-archived personas, ordered by name.
    async fn list(&self) -> PersonaResult<Vec<Persona>>;

    /// Fetch a single persona by its UUID. Returns `NotFound` if absent.
    async fn get(&self, id: Uuid) -> PersonaResult<Persona>;

    /// Insert a new persona. Returns `DuplicateName` if `name` already exists.
    async fn create(&self, req: &CreatePersonaRequest) -> PersonaResult<Persona>;

    /// Apply non-`None` fields from `req` to the persona identified by `id`.
    /// Returns the updated persona. Returns `NotFound` if no matching row exists.
    async fn update(&self, id: Uuid, req: &UpdatePersonaRequest) -> PersonaResult<Persona>;

    /// Soft-delete a persona. Existing session attachments are retained but the
    /// persona cannot be attached to new sessions. Idempotent.
    async fn archive_persona(&self, id: Uuid) -> PersonaResult<()>;

    // ── Session attachment ────────────────────────────────────────────────────

    /// Attach `persona_id` to `session_id`, replacing any previous attachment.
    /// Returns `Archived` if the persona has been soft-deleted.
    async fn attach_persona_to_session(
        &self,
        session_id: &str,
        persona_id: Uuid,
    ) -> PersonaResult<SessionPersona>;

    /// Detach any persona currently attached to `session_id`. Idempotent —
    /// returns `Ok(())` even when no attachment exists.
    async fn detach_persona_from_session(&self, session_id: &str) -> PersonaResult<()>;

    /// Return the persona (if any) currently attached to `session_id`.
    async fn get_session_persona(&self, session_id: &str) -> PersonaResult<Option<Persona>>;
}
