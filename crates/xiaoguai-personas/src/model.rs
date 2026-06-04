//! Domain types for agent persona profiles.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A named role profile that shapes agent behaviour.
///
/// The persona is injected at chat-time: its `system_prompt` is prepended to
/// the message history and its `tool_allowlist` gates which MCP/toolbox tools
/// the agent may invoke. A `None` allowlist means "no restriction" (every
/// tool); an *empty* allowlist means "no tools allowed".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Persona {
    pub id: Uuid,
    /// Human-readable label. Unique by name (enforced at DB level).
    pub name: String,
    /// Injected as the leading system message in every chat turn.
    pub system_prompt: String,
    /// Optional model override. `None` = use the session / global default.
    pub default_model: Option<String>,
    /// `None` = unrestricted (all tools). `Some([])` = no tools.
    pub tool_allowlist: Option<Vec<String>>,
    /// Opaque escalation tier label for HOTL integration (e.g. "L1", "human").
    pub escalation_tier: Option<String>,
    pub created_at: DateTime<Utc>,
    /// Soft-deleted personas cannot be attached to new sessions.
    pub archived: bool,
}

impl Persona {
    /// Return `true` when `tool_name` is permitted by this persona's allowlist.
    ///
    /// - `None` allowlist → all tools allowed.
    /// - `Some(list)` → only tools whose name appears in `list` are allowed.
    #[must_use]
    pub fn allows_tool(&self, tool_name: &str) -> bool {
        match &self.tool_allowlist {
            None => true,
            Some(list) => list.iter().any(|t| t == tool_name),
        }
    }
}

/// Payload used when creating a new persona.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatePersonaRequest {
    pub name: String,
    #[serde(default)]
    pub system_prompt: String,
    pub default_model: Option<String>,
    /// `None` = unrestricted, `Some([])` = deny all tools.
    pub tool_allowlist: Option<Vec<String>>,
    pub escalation_tier: Option<String>,
}

/// Payload used when updating an existing persona.
///
/// Only non-`None` fields are applied; the others retain their current value.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdatePersonaRequest {
    pub name: Option<String>,
    pub system_prompt: Option<String>,
    /// `None` = do not change. `Some(None)` = clear (unrestricted).
    /// `Some(Some([]))` = deny all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_allowlist: Option<Option<Vec<String>>>,
    pub default_model: Option<String>,
    pub escalation_tier: Option<String>,
}

/// Records which persona is attached to a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionPersona {
    pub session_id: String,
    pub persona_id: Uuid,
    pub attached_at: DateTime<Utc>,
}
