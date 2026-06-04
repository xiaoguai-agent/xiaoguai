//! `SQLite`-backed `PersonaRepository` implementation (DEC-033 single-user pivot).
//!
//! Single-namespace store: the multi-tenant `tenant_id` columns are gone. The
//! `personas` table has a partial index on `name` (active rows) so list +
//! name-lookup queries stay cheap. `session_personas` has `session_id` as its
//! PRIMARY KEY which enforces the one-persona-per-session invariant at the DB
//! level; the upsert path in [`SqlitePersonaRepository::attach_persona_to_session`]
//! relies on this via `ON CONFLICT DO UPDATE`.
//!
//! Postgres `tool_allowlist TEXT[]` is now stored as TEXT holding a JSON array
//! (NULL or `'[]'` = empty). Reads parse the JSON; writes serialize via
//! `serde_json`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

use crate::error::{PersonaError, PersonaResult};
use crate::model::{CreatePersonaRequest, Persona, SessionPersona, UpdatePersonaRequest};
use crate::traits::PersonaRepository;

/// `SQLite` implementation. Clone is cheap — `SqlitePool` is an `Arc` internally.
#[derive(Debug, Clone)]
pub struct SqlitePersonaRepository {
    pool: SqlitePool,
}

impl SqlitePersonaRepository {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

// ── JSON-array <-> Vec<String> helpers (TEXT[] replacement) ──────────────────

/// Serialize an optional allowlist to JSON-array TEXT for storage.
///
/// `None` → SQL NULL (unrestricted). `Some(list)` → JSON array text (`'[]'`
/// when empty = deny all).
fn allowlist_to_text(list: Option<&Vec<String>>) -> Option<String> {
    list.map(|l| serde_json::to_string(l).unwrap_or_else(|_| "[]".to_string()))
}

/// Parse stored JSON-array TEXT back into an optional allowlist.
///
/// NULL → `None` (unrestricted). `'[]'` → `Some(vec![])` (deny all). Malformed
/// text degrades to `Some(vec![])` rather than failing the read.
fn text_to_allowlist(text: Option<String>) -> Option<Vec<String>> {
    text.map(|t| serde_json::from_str::<Vec<String>>(&t).unwrap_or_default())
}

// ── Row types (sqlx deserialization) ─────────────────────────────────────────

#[derive(Debug, FromRow)]
struct PersonaRow {
    // SQLite stores the UUID as TEXT; parse into `Uuid` in `From`.
    id: String,
    name: String,
    system_prompt: String,
    default_model: Option<String>,
    // Postgres TEXT[] → SQLite TEXT holding a JSON array.
    tool_allowlist: Option<String>,
    escalation_tier: Option<String>,
    created_at: DateTime<Utc>,
    archived: bool,
}

impl From<PersonaRow> for Persona {
    fn from(r: PersonaRow) -> Self {
        Self {
            id: Uuid::parse_str(&r.id).unwrap_or_else(|_| Uuid::nil()),
            name: r.name,
            system_prompt: r.system_prompt,
            default_model: r.default_model,
            tool_allowlist: text_to_allowlist(r.tool_allowlist),
            escalation_tier: r.escalation_tier,
            created_at: r.created_at,
            archived: r.archived,
        }
    }
}

#[derive(Debug, FromRow)]
struct SessionPersonaRow {
    session_id: String,
    persona_id: String,
    attached_at: DateTime<Utc>,
}

impl From<SessionPersonaRow> for SessionPersona {
    fn from(r: SessionPersonaRow) -> Self {
        Self {
            session_id: r.session_id,
            persona_id: Uuid::parse_str(&r.persona_id).unwrap_or_else(|_| Uuid::nil()),
            attached_at: r.attached_at,
        }
    }
}

// ── Trait implementation ──────────────────────────────────────────────────────

#[async_trait]
impl PersonaRepository for SqlitePersonaRepository {
    async fn list(&self) -> PersonaResult<Vec<Persona>> {
        let rows: Vec<PersonaRow> = sqlx::query_as(
            "SELECT id, name, system_prompt, default_model, \
                    tool_allowlist, escalation_tier, created_at, archived \
             FROM personas \
             WHERE NOT archived \
             ORDER BY name ASC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(PersonaError::from_sqlx)?;
        Ok(rows.into_iter().map(Persona::from).collect())
    }

    async fn get(&self, id: Uuid) -> PersonaResult<Persona> {
        let row: Option<PersonaRow> = sqlx::query_as(
            "SELECT id, name, system_prompt, default_model, \
                    tool_allowlist, escalation_tier, created_at, archived \
             FROM personas WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(PersonaError::from_sqlx)?;
        row.map(Persona::from).ok_or(PersonaError::NotFound)
    }

    async fn create(&self, req: &CreatePersonaRequest) -> PersonaResult<Persona> {
        let id = Uuid::new_v4();
        let now = Utc::now();
        let allowlist = allowlist_to_text(req.tool_allowlist.as_ref());
        let row: PersonaRow = sqlx::query_as(
            "INSERT INTO personas \
               (id, name, system_prompt, default_model, \
                tool_allowlist, escalation_tier, created_at, archived) \
             VALUES (?, ?, ?, ?, ?, ?, ?, false) \
             RETURNING id, name, system_prompt, default_model, \
                       tool_allowlist, escalation_tier, created_at, archived",
        )
        .bind(id.to_string())
        .bind(&req.name)
        .bind(&req.system_prompt)
        .bind(&req.default_model)
        .bind(allowlist)
        .bind(&req.escalation_tier)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(PersonaError::from_sqlx)?;
        Ok(Persona::from(row))
    }

    async fn update(&self, id: Uuid, req: &UpdatePersonaRequest) -> PersonaResult<Persona> {
        // Fetch current state first so we can apply partial updates correctly.
        let current = self.get(id).await?;

        let new_name = req.name.as_deref().unwrap_or(&current.name);
        let new_prompt = req
            .system_prompt
            .as_deref()
            .unwrap_or(&current.system_prompt);
        // tool_allowlist: None = keep current; Some(inner) = replace with inner.
        let new_allowlist: Option<Vec<String>> = match &req.tool_allowlist {
            None => current.tool_allowlist.clone(),
            Some(inner) => inner.clone(),
        };
        let new_allowlist_text = allowlist_to_text(new_allowlist.as_ref());
        let new_model: Option<&str> = req
            .default_model
            .as_deref()
            .or(current.default_model.as_deref());
        let new_tier: Option<&str> = req
            .escalation_tier
            .as_deref()
            .or(current.escalation_tier.as_deref());

        let row: PersonaRow = sqlx::query_as(
            "UPDATE personas \
             SET name = ?, system_prompt = ?, default_model = ?, \
                 tool_allowlist = ?, escalation_tier = ? \
             WHERE id = ? \
             RETURNING id, name, system_prompt, default_model, \
                       tool_allowlist, escalation_tier, created_at, archived",
        )
        .bind(new_name)
        .bind(new_prompt)
        .bind(new_model)
        .bind(new_allowlist_text)
        .bind(new_tier)
        .bind(id.to_string())
        .fetch_one(&self.pool)
        .await
        .map_err(PersonaError::from_sqlx)?;
        Ok(Persona::from(row))
    }

    async fn archive_persona(&self, id: Uuid) -> PersonaResult<()> {
        sqlx::query("UPDATE personas SET archived = true WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .map_err(PersonaError::from_sqlx)?;
        // Idempotent — no error if already archived or row doesn't exist.
        Ok(())
    }

    async fn attach_persona_to_session(
        &self,
        session_id: &str,
        persona_id: Uuid,
    ) -> PersonaResult<SessionPersona> {
        // Guard: refuse to attach archived personas.
        let persona = self.get(persona_id).await?;
        if persona.archived {
            return Err(PersonaError::Archived);
        }

        let now = Utc::now();
        // Upsert: replace any existing attachment for this session.
        let row: SessionPersonaRow = sqlx::query_as(
            "INSERT INTO session_personas (session_id, persona_id, attached_at) \
             VALUES (?, ?, ?) \
             ON CONFLICT (session_id) \
             DO UPDATE SET persona_id = EXCLUDED.persona_id, \
                           attached_at = EXCLUDED.attached_at \
             RETURNING session_id, persona_id, attached_at",
        )
        .bind(session_id)
        .bind(persona_id.to_string())
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(PersonaError::from_sqlx)?;
        Ok(SessionPersona::from(row))
    }

    async fn detach_persona_from_session(&self, session_id: &str) -> PersonaResult<()> {
        sqlx::query("DELETE FROM session_personas WHERE session_id = ?")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(PersonaError::from_sqlx)?;
        Ok(())
    }

    async fn get_session_persona(&self, session_id: &str) -> PersonaResult<Option<Persona>> {
        let row: Option<PersonaRow> = sqlx::query_as(
            "SELECT p.id, p.name, p.system_prompt, p.default_model, \
                    p.tool_allowlist, p.escalation_tier, p.created_at, p.archived \
             FROM session_personas sp \
             JOIN personas p ON p.id = sp.persona_id \
             WHERE sp.session_id = ?",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(PersonaError::from_sqlx)?;
        Ok(row.map(Persona::from))
    }
}
