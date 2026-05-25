//! Discord Interactions payload parsing and response construction.
//!
//! Discord sends all Interactions (slash commands, button clicks, etc.)
//! as `POST` requests to the configured Interactions Endpoint URL.  The
//! request body is JSON with a top-level `type` integer discriminant:
//!
//! | type | name                    | handled here |
//! |------|-------------------------|-------------|
//! | 1    | `PING`                  | ✅ — PONG immediately |
//! | 2    | `APPLICATION_COMMAND`   | ✅ — slash command → `ImEvent::Message` |
//! | 3    | `MESSAGE_COMPONENT`     | ✅ — button / select menu |
//! | 4+   | (autocomplete, modal, etc.) | 400 Malformed |
//!
//! All field names match the Discord API v10 Interaction object.
//! <https://discord.com/developers/docs/interactions/receiving-and-responding>

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use xiaoguai_im_gateway::{ImEvent, IncomingMessage, ProviderError};

// ── Interaction type discriminants ──────────────────────────────────────────

pub const TYPE_PING: u8 = 1;
pub const TYPE_APPLICATION_COMMAND: u8 = 2;
pub const TYPE_MESSAGE_COMPONENT: u8 = 3;

// ── Response type discriminants ─────────────────────────────────────────────

pub const RESPONSE_PONG: u8 = 1;
pub const RESPONSE_CHANNEL_MESSAGE: u8 = 4;

// ── Wire types ───────────────────────────────────────────────────────────────

/// Minimal Discord Interaction envelope. We ignore fields that xiaoguai
/// doesn't use (e.g. `guild_id`, locale, permissions).
#[derive(Debug, Clone, Deserialize)]
pub struct Interaction {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: u8,
    /// Present for `APPLICATION_COMMAND` (type 2).
    #[serde(default)]
    pub data: Option<InteractionData>,
    /// Discord guild (server) id — absent for DMs.
    #[serde(default)]
    pub guild_id: Option<String>,
    /// Channel id — present for guild interactions.
    #[serde(default)]
    pub channel_id: Option<String>,
    /// Member object (in guilds).
    #[serde(default)]
    pub member: Option<Member>,
    /// User object (in DMs or from `member.user`).
    #[serde(default)]
    pub user: Option<User>,
    /// Application id (bot's application id).
    #[serde(default)]
    pub application_id: Option<String>,
    /// Guild/DM channel token — used to send followup messages.
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InteractionData {
    /// Command or component custom id.
    pub id: Option<String>,
    /// Slash command name.
    #[serde(default)]
    pub name: Option<String>,
    /// Resolved options for slash commands.
    #[serde(default)]
    pub options: Vec<CommandOption>,
    /// Custom id for `MESSAGE_COMPONENT` interactions.
    #[serde(default)]
    pub custom_id: Option<String>,
    /// Component type for `MESSAGE_COMPONENT` interactions.
    #[serde(default)]
    pub component_type: Option<u8>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommandOption {
    pub name: String,
    #[serde(default)]
    pub value: Option<JsonValue>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Member {
    pub user: Option<User>,
    /// Server nickname — not always present.
    #[serde(default)]
    pub nick: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct User {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
}

// ── Response helpers ─────────────────────────────────────────────────────────

/// Build a PONG response (required for Discord to validate the endpoint).
#[must_use]
pub fn pong_response() -> JsonValue {
    json!({ "type": RESPONSE_PONG })
}

/// Build a plain-text channel message response.
#[must_use]
pub fn message_response(content: impl Into<String>) -> JsonValue {
    json!({
        "type": RESPONSE_CHANNEL_MESSAGE,
        "data": { "content": content.into() }
    })
}

// ── Payload parsing ───────────────────────────────────────────────────────────

/// Parse a raw JSON body into an [`ImEvent`] (or return the PONG JSON for
/// PING interactions).
///
/// Returns `Ok(None)` for a PING — the caller must respond with
/// [`pong_response()`] immediately; no agent turn is needed.
/// Returns `Ok(Some(ImEvent::Message(_)))` for slash commands and button
/// clicks that map to a text conversation.
///
/// # Errors
///
/// Returns [`ProviderError::Malformed`] for unknown interaction types or
/// structural issues.
pub fn parse_interaction(body: &[u8]) -> Result<Option<ImEvent>, ProviderError> {
    let interaction: Interaction = serde_json::from_slice(body)
        .map_err(|e| ProviderError::Malformed(format!("decode interaction: {e}")))?;

    match interaction.kind {
        TYPE_PING => Ok(None),
        TYPE_APPLICATION_COMMAND => {
            let event = parse_command(&interaction)?;
            Ok(Some(event))
        }
        TYPE_MESSAGE_COMPONENT => {
            let event = parse_component(&interaction)?;
            Ok(Some(event))
        }
        other => Err(ProviderError::Malformed(format!(
            "unsupported interaction type: {other}"
        ))),
    }
}

/// Extract the user from member.user (guild) or top-level user (DM).
fn resolve_user(interaction: &Interaction) -> Result<&User, ProviderError> {
    interaction
        .member
        .as_ref()
        .and_then(|m| m.user.as_ref())
        .or(interaction.user.as_ref())
        .ok_or_else(|| ProviderError::Malformed("no user in interaction".into()))
}

/// Produce a stable conversation id for Discord interactions.
///
/// Guild interactions are scoped to `channel_id`; DMs use `user_id`.
fn conversation_id(interaction: &Interaction, user: &User) -> String {
    match interaction.channel_id.as_deref() {
        Some(ch) => format!("discord:channel:{ch}"),
        None => format!("discord:dm:{}", user.id),
    }
}

/// Derive the tenant id from the guild id (or "dm" for direct messages).
fn tenant_id(interaction: &Interaction) -> String {
    interaction
        .guild_id
        .clone()
        .unwrap_or_else(|| "dm".to_string())
}

fn parse_command(interaction: &Interaction) -> Result<ImEvent, ProviderError> {
    let data = interaction
        .data
        .as_ref()
        .ok_or_else(|| ProviderError::Malformed("APPLICATION_COMMAND missing data".into()))?;
    let command_name = data
        .name
        .as_deref()
        .ok_or_else(|| ProviderError::Malformed("command missing name".into()))?;

    // Build a text representation: `/command_name option1=val1 …`
    let mut text = format!("/{command_name}");
    for opt in &data.options {
        if let Some(val) = &opt.value {
            let val_str = match val {
                JsonValue::String(s) => s.clone(),
                other => other.to_string(),
            };
            text.push(' ');
            text.push_str(&opt.name);
            text.push('=');
            text.push_str(&val_str);
        }
    }

    let user = resolve_user(interaction)?;

    Ok(ImEvent::Message(IncomingMessage {
        provider: "discord".into(),
        user_external_id: user.id.clone(),
        tenant_external_id: tenant_id(interaction),
        conversation_id: conversation_id(interaction, user),
        text,
        event_id: interaction.id.clone(),
    }))
}

fn parse_component(interaction: &Interaction) -> Result<ImEvent, ProviderError> {
    let data = interaction
        .data
        .as_ref()
        .ok_or_else(|| ProviderError::Malformed("MESSAGE_COMPONENT missing data".into()))?;
    let custom_id = data
        .custom_id
        .as_deref()
        .ok_or_else(|| ProviderError::Malformed("component missing custom_id".into()))?;

    // Represent a button click as a synthetic text turn so the agent can
    // respond contextually: "[component] <custom_id>"
    let text = format!("[component] {custom_id}");
    let user = resolve_user(interaction)?;

    Ok(ImEvent::Message(IncomingMessage {
        provider: "discord".into(),
        user_external_id: user.id.clone(),
        tenant_external_id: tenant_id(interaction),
        conversation_id: conversation_id(interaction, user),
        text,
        event_id: interaction.id.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── PING ────────────────────────────────────────────────────────────────

    #[test]
    fn ping_returns_none() {
        let body = br#"{"id":"1","type":1,"application_id":"app1"}"#;
        let result = parse_interaction(body).expect("parse");
        assert!(result.is_none(), "PING should return None");
    }

    #[test]
    fn pong_response_shape() {
        let resp = pong_response();
        assert_eq!(resp["type"], RESPONSE_PONG);
    }

    // ── APPLICATION_COMMAND ─────────────────────────────────────────────────

    #[test]
    fn slash_command_no_options_parses() {
        let body = json!({
            "id": "cmd1",
            "type": TYPE_APPLICATION_COMMAND,
            "guild_id": "guild_x",
            "channel_id": "ch_y",
            "member": { "user": { "id": "user_u", "username": "alice" } },
            "data": { "id": "d1", "name": "ask", "options": [] }
        });
        let event = parse_interaction(body.to_string().as_bytes())
            .expect("parse")
            .expect("some event");
        let ImEvent::Message(m) = event else {
            panic!("expected Message");
        };
        assert_eq!(m.provider, "discord");
        assert_eq!(m.user_external_id, "user_u");
        assert_eq!(m.tenant_external_id, "guild_x");
        assert_eq!(m.conversation_id, "discord:channel:ch_y");
        assert_eq!(m.text, "/ask");
        assert_eq!(m.event_id, "cmd1");
    }

    #[test]
    fn slash_command_with_options_builds_text() {
        let body = json!({
            "id": "cmd2",
            "type": TYPE_APPLICATION_COMMAND,
            "guild_id": "g",
            "channel_id": "ch",
            "member": { "user": { "id": "u2", "username": "bob" } },
            "data": {
                "id": "d2",
                "name": "query",
                "options": [
                    { "name": "topic", "value": "rust" },
                    { "name": "limit", "value": 5 }
                ]
            }
        });
        let event = parse_interaction(body.to_string().as_bytes())
            .expect("parse")
            .expect("some event");
        let ImEvent::Message(m) = event else {
            panic!("expected Message");
        };
        assert_eq!(m.text, "/query topic=rust limit=5");
    }

    #[test]
    fn dm_slash_command_uses_user_not_member() {
        let body = json!({
            "id": "cmd3",
            "type": TYPE_APPLICATION_COMMAND,
            "user": { "id": "dm_user", "username": "carol" },
            "data": { "id": "d3", "name": "help", "options": [] }
        });
        let event = parse_interaction(body.to_string().as_bytes())
            .expect("parse")
            .expect("some event");
        let ImEvent::Message(m) = event else {
            panic!("expected Message");
        };
        assert_eq!(m.user_external_id, "dm_user");
        assert_eq!(m.tenant_external_id, "dm");
        assert_eq!(m.conversation_id, "discord:dm:dm_user");
        assert_eq!(m.text, "/help");
    }

    // ── MESSAGE_COMPONENT ───────────────────────────────────────────────────

    #[test]
    fn button_click_parses() {
        let body = json!({
            "id": "cmp1",
            "type": TYPE_MESSAGE_COMPONENT,
            "guild_id": "g",
            "channel_id": "ch",
            "member": { "user": { "id": "u3", "username": "dave" } },
            "data": { "custom_id": "approve_btn", "component_type": 2 }
        });
        let event = parse_interaction(body.to_string().as_bytes())
            .expect("parse")
            .expect("some event");
        let ImEvent::Message(m) = event else {
            panic!("expected Message");
        };
        assert_eq!(m.text, "[component] approve_btn");
        assert_eq!(m.event_id, "cmp1");
    }

    // ── Error cases ─────────────────────────────────────────────────────────

    #[test]
    fn unknown_interaction_type_errors() {
        let body = br#"{"id":"x","type":99}"#;
        assert!(matches!(
            parse_interaction(body),
            Err(ProviderError::Malformed(_))
        ));
    }

    #[test]
    fn malformed_json_errors() {
        assert!(matches!(
            parse_interaction(b"not json"),
            Err(ProviderError::Malformed(_))
        ));
    }

    #[test]
    fn command_missing_data_errors() {
        let body = json!({
            "id": "x",
            "type": TYPE_APPLICATION_COMMAND,
            "member": { "user": { "id": "u", "username": "u" } }
        });
        assert!(matches!(
            parse_interaction(body.to_string().as_bytes()),
            Err(ProviderError::Malformed(_))
        ));
    }

    #[test]
    fn component_missing_custom_id_errors() {
        let body = json!({
            "id": "y",
            "type": TYPE_MESSAGE_COMPONENT,
            "member": { "user": { "id": "u", "username": "u" } },
            "data": { "component_type": 2 }
        });
        assert!(matches!(
            parse_interaction(body.to_string().as_bytes()),
            Err(ProviderError::Malformed(_))
        ));
    }

    // ── Response helpers ─────────────────────────────────────────────────────

    #[test]
    fn message_response_shape() {
        let resp = message_response("hello world");
        assert_eq!(resp["type"], RESPONSE_CHANNEL_MESSAGE);
        assert_eq!(resp["data"]["content"], "hello world");
    }
}
