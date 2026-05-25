//! In-memory `PersonaRepository` for unit tests and integration harnesses.
//!
//! Not intended for production use. State is held behind a `parking_lot::Mutex`
//! so the type is `Send + Sync` and works in async test contexts.

use async_trait::async_trait;
use chrono::Utc;
use parking_lot::Mutex;
use std::collections::HashMap;
use uuid::Uuid;

use crate::error::{PersonaError, PersonaResult};
use crate::model::{CreatePersonaRequest, Persona, SessionPersona, UpdatePersonaRequest};
use crate::traits::PersonaRepository;

#[derive(Default)]
struct Inner {
    personas: HashMap<Uuid, Persona>,
    /// `session_id` → `persona_id`
    attachments: HashMap<String, Uuid>,
}

/// Thread-safe in-memory store. All operations are synchronous under the mutex.
#[derive(Default)]
pub struct InMemoryPersonaRepository {
    state: Mutex<Inner>,
}

impl InMemoryPersonaRepository {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl PersonaRepository for InMemoryPersonaRepository {
    async fn list(&self, tenant_id: Uuid) -> PersonaResult<Vec<Persona>> {
        let g = self.state.lock();
        let mut out: Vec<Persona> = g
            .personas
            .values()
            .filter(|p| p.tenant_id == tenant_id && !p.archived)
            .cloned()
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn get(&self, id: Uuid) -> PersonaResult<Persona> {
        let g = self.state.lock();
        g.personas.get(&id).cloned().ok_or(PersonaError::NotFound)
    }

    async fn create(&self, req: &CreatePersonaRequest) -> PersonaResult<Persona> {
        let mut g = self.state.lock();
        // Enforce (tenant_id, name) uniqueness.
        let duplicate = g
            .personas
            .values()
            .any(|p| p.tenant_id == req.tenant_id && p.name == req.name && !p.archived);
        if duplicate {
            return Err(PersonaError::DuplicateName(req.name.clone()));
        }
        let persona = Persona {
            id: Uuid::new_v4(),
            tenant_id: req.tenant_id,
            name: req.name.clone(),
            system_prompt: req.system_prompt.clone(),
            default_model: req.default_model.clone(),
            tool_allowlist: req.tool_allowlist.clone(),
            escalation_tier: req.escalation_tier.clone(),
            created_at: Utc::now(),
            archived: false,
        };
        g.personas.insert(persona.id, persona.clone());
        Ok(persona)
    }

    async fn update(&self, id: Uuid, req: &UpdatePersonaRequest) -> PersonaResult<Persona> {
        let mut g = self.state.lock();
        let persona = g.personas.get_mut(&id).ok_or(PersonaError::NotFound)?;
        if let Some(name) = &req.name {
            name.clone_into(&mut persona.name);
        }
        if let Some(prompt) = &req.system_prompt {
            prompt.clone_into(&mut persona.system_prompt);
        }
        if let Some(allowlist) = &req.tool_allowlist {
            persona.tool_allowlist.clone_from(allowlist);
        }
        if let Some(model) = &req.default_model {
            persona.default_model = Some(model.clone());
        }
        if let Some(tier) = &req.escalation_tier {
            persona.escalation_tier = Some(tier.clone());
        }
        Ok(persona.clone())
    }

    async fn archive_persona(&self, id: Uuid) -> PersonaResult<()> {
        let mut g = self.state.lock();
        if let Some(p) = g.personas.get_mut(&id) {
            p.archived = true;
        }
        Ok(())
    }

    async fn attach_persona_to_session(
        &self,
        session_id: &str,
        persona_id: Uuid,
    ) -> PersonaResult<SessionPersona> {
        let mut g = self.state.lock();
        let persona = g
            .personas
            .get(&persona_id)
            .ok_or(PersonaError::NotFound)?;
        if persona.archived {
            return Err(PersonaError::Archived);
        }
        let now = Utc::now();
        g.attachments.insert(session_id.to_string(), persona_id);
        Ok(SessionPersona {
            session_id: session_id.to_string(),
            persona_id,
            attached_at: now,
        })
    }

    async fn detach_persona_from_session(&self, session_id: &str) -> PersonaResult<()> {
        self.state.lock().attachments.remove(session_id);
        Ok(())
    }

    async fn get_session_persona(&self, session_id: &str) -> PersonaResult<Option<Persona>> {
        let g = self.state.lock();
        let persona_id = match g.attachments.get(session_id) {
            Some(id) => *id,
            None => return Ok(None),
        };
        Ok(g.personas.get(&persona_id).cloned())
    }
}
