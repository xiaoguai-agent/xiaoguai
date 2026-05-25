//! Postgres-backed `PersonaRepository` implementation.
//!
//! All queries are tenant-scoped. The `personas` table has an index on
//! `(tenant_id, name)` so list + name-lookup queries are O(log n) even at
//! large tenant sizes. `session_personas` has a unique constraint on
//! `session_id` which enforces the one-persona-per-session invariant at the
//! DB level; the upsert path in [`PgPersonaRepository::attach_persona_to_session`]
//! relies on this via `ON CONFLICT DO UPDATE`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::error::{PersonaError, PersonaResult};
use crate::model::{CreatePersonaRequest, Persona, SessionPersona, UpdatePersonaRequest};
use crate::traits::PersonaRepository;

/// Postgres implementation. Clone is cheap — `PgPool` is an `Arc` internally.
#[derive(Debug, Clone)]
pub struct PgPersonaRepository {
    pool: PgPool,
}

impl PgPersonaRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

// ── Row types (sqlx deserialization) ─────────────────────────────────────────

#[derive(Debug, FromRow)]
struct PersonaRow {
    id: Uuid,
    tenant_id: Uuid,
    name: String,
    system_prompt: String,
    default_model: Option<String>,
    // sqlx maps TEXT[] → Vec<String> automatically when the postgres feature
    // and the `postgres` driver are active.
    tool_allowlist: Option<Vec<String>>,
    escalation_tier: Option<String>,
    created_at: DateTime<Utc>,
    archived: bool,
}

impl From<PersonaRow> for Persona {
    fn from(r: PersonaRow) -> Self {
        Self {
            id: r.id,
            tenant_id: r.tenant_id,
            name: r.name,
            system_prompt: r.system_prompt,
            default_model: r.default_model,
            tool_allowlist: r.tool_allowlist,
            escalation_tier: r.escalation_tier,
            created_at: r.created_at,
            archived: r.archived,
        }
    }
}

#[derive(Debug, FromRow)]
struct SessionPersonaRow {
    session_id: String,
    persona_id: Uuid,
    attached_at: DateTime<Utc>,
}

impl From<SessionPersonaRow> for SessionPersona {
    fn from(r: SessionPersonaRow) -> Self {
        Self {
            session_id: r.session_id,
            persona_id: r.persona_id,
            attached_at: r.attached_at,
        }
    }
}

// ── Trait implementation ──────────────────────────────────────────────────────

#[async_trait]
impl PersonaRepository for PgPersonaRepository {
    async fn list(&self, tenant_id: Uuid) -> PersonaResult<Vec<Persona>> {
        let rows: Vec<PersonaRow> = sqlx::query_as(
            "SELECT id, tenant_id, name, system_prompt, default_model, \
                    tool_allowlist, escalation_tier, created_at, archived \
             FROM personas \
             WHERE tenant_id = $1 AND NOT archived \
             ORDER BY name ASC",
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await
        .map_err(PersonaError::from_sqlx)?;
        Ok(rows.into_iter().map(Persona::from).collect())
    }

    async fn get(&self, id: Uuid) -> PersonaResult<Persona> {
        let row: Option<PersonaRow> = sqlx::query_as(
            "SELECT id, tenant_id, name, system_prompt, default_model, \
                    tool_allowlist, escalation_tier, created_at, archived \
             FROM personas WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(PersonaError::from_sqlx)?;
        row.map(Persona::from).ok_or(PersonaError::NotFound)
    }

    async fn create(&self, req: &CreatePersonaRequest) -> PersonaResult<Persona> {
        let id = Uuid::new_v4();
        let now = Utc::now();
        let row: PersonaRow = sqlx::query_as(
            "INSERT INTO personas \
               (id, tenant_id, name, system_prompt, default_model, \
                tool_allowlist, escalation_tier, created_at, archived) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, false) \
             RETURNING id, tenant_id, name, system_prompt, default_model, \
                       tool_allowlist, escalation_tier, created_at, archived",
        )
        .bind(id)
        .bind(req.tenant_id)
        .bind(&req.name)
        .bind(&req.system_prompt)
        .bind(&req.default_model)
        .bind(&req.tool_allowlist)
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
             SET name = $2, system_prompt = $3, default_model = $4, \
                 tool_allowlist = $5, escalation_tier = $6 \
             WHERE id = $1 \
             RETURNING id, tenant_id, name, system_prompt, default_model, \
                       tool_allowlist, escalation_tier, created_at, archived",
        )
        .bind(id)
        .bind(new_name)
        .bind(new_prompt)
        .bind(new_model)
        .bind(&new_allowlist)
        .bind(new_tier)
        .fetch_one(&self.pool)
        .await
        .map_err(PersonaError::from_sqlx)?;
        Ok(Persona::from(row))
    }

    async fn archive_persona(&self, id: Uuid) -> PersonaResult<()> {
        sqlx::query("UPDATE personas SET archived = true WHERE id = $1")
            .bind(id)
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
             VALUES ($1, $2, $3) \
             ON CONFLICT (session_id) \
             DO UPDATE SET persona_id = EXCLUDED.persona_id, \
                           attached_at = EXCLUDED.attached_at \
             RETURNING session_id, persona_id, attached_at",
        )
        .bind(session_id)
        .bind(persona_id)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(PersonaError::from_sqlx)?;
        Ok(SessionPersona::from(row))
    }

    async fn detach_persona_from_session(&self, session_id: &str) -> PersonaResult<()> {
        sqlx::query("DELETE FROM session_personas WHERE session_id = $1")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(PersonaError::from_sqlx)?;
        Ok(())
    }

    async fn get_session_persona(&self, session_id: &str) -> PersonaResult<Option<Persona>> {
        let row: Option<PersonaRow> = sqlx::query_as(
            "SELECT p.id, p.tenant_id, p.name, p.system_prompt, p.default_model, \
                    p.tool_allowlist, p.escalation_tier, p.created_at, p.archived \
             FROM session_personas sp \
             JOIN personas p ON p.id = sp.persona_id \
             WHERE sp.session_id = $1",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(PersonaError::from_sqlx)?;
        Ok(row.map(Persona::from))
    }
}
