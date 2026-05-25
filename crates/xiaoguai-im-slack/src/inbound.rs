//! Slack Events API payload parsing.
//!
//! Handles three top-level shapes:
//!
//! 1. **URL verification challenge** — `{"type":"url_verification","challenge":"<token>"}`
//! 2. **Event callback** — `{"type":"event_callback","event":{...},...}` covering:
//!    - `message` (channel text message or DM)
//!    - `app_mention` (user @-mentions the bot)
//!    - `app_home_opened` (user opens the App Home tab)
//!    - `reaction_added` (user reacts to a message)
//! 3. Unknown `type` values → `ProviderError::Malformed`.
//!
//! Bot messages (those containing `bot_id`) and retries sent by Slack
//! when the app doesn't respond fast enough (`X-Slack-Retry-Num` > 0)
//! are silently ignored via the `ImEvent::Ignored` variant.

use serde::Deserialize;
use serde_json::Value as JsonValue;

use xiaoguai_im_gateway::{ImEvent, IncomingMessage, ProviderError};

// ── top-level envelope ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    challenge: Option<String>,
    #[serde(default)]
    team_id: Option<String>,
    #[serde(default)]
    event: Option<EventBody>,
    #[serde(default)]
    event_id: Option<String>,
}

// ── inner event body ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct EventBody {
    #[serde(rename = "type")]
    kind: String,
    // message / app_mention fields
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    ts: Option<String>,
    /// Bot messages carry `bot_id`; we drop those to avoid loops.
    #[serde(default)]
    bot_id: Option<String>,

    // reaction_added fields
    #[serde(default)]
    reaction: Option<String>,
    #[serde(default)]
    item: Option<ReactionItem>,

    // app_home_opened fields
    #[serde(default)]
    tab: Option<String>,
}

#[derive(Deserialize)]
struct ReactionItem {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    kind: String,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    ts: Option<String>,
}

// ── public parse function ─────────────────────────────────────────────────────

/// Parse the raw webhook body into an [`ImEvent`].
///
/// Callers must verify the signature before calling this.
///
/// # Errors
/// Returns [`ProviderError::Malformed`] for unknown envelope types or
/// missing required fields. Returns [`ProviderError::BadSignature`] is
/// *not* returned here — signature checking is the caller's responsibility.
pub fn parse_event(body: &[u8], retry_num: Option<&str>) -> Result<ImEvent, ProviderError> {
    // Slack retries the delivery if the app takes >3 s to respond.
    // Silently discard retries so we don't process the same message twice.
    if retry_num.is_some_and(|v| v != "0") {
        return Ok(ImEvent::Ignored);
    }

    let envelope: Envelope = serde_json::from_slice(body)
        .map_err(|e| ProviderError::Malformed(format!("decode envelope: {e}")))?;

    match envelope.kind.as_str() {
        "url_verification" => {
            let challenge = envelope.challenge.ok_or_else(|| {
                ProviderError::Malformed("url_verification missing challenge".into())
            })?;
            Ok(ImEvent::Challenge { challenge })
        }
        "event_callback" => parse_event_callback(envelope),
        other => Err(ProviderError::Malformed(format!(
            "unknown envelope type: {other}"
        ))),
    }
}

fn parse_event_callback(env: Envelope) -> Result<ImEvent, ProviderError> {
    let team_id = env.team_id.unwrap_or_default();
    let event_id = env.event_id.unwrap_or_default();
    let event = env
        .event
        .ok_or_else(|| ProviderError::Malformed("event_callback missing 'event'".into()))?;

    // Drop bot-originated messages to avoid infinite reply loops.
    if event.bot_id.is_some() {
        return Ok(ImEvent::Ignored);
    }

    match event.kind.as_str() {
        "message" | "app_mention" => {
            let channel = event
                .channel
                .ok_or_else(|| ProviderError::Malformed("event missing channel".into()))?;
            let user = event
                .user
                .ok_or_else(|| ProviderError::Malformed("event missing user".into()))?;
            let text = event.text.unwrap_or_default();
            let ts = event.ts.unwrap_or_default();
            // Use the Slack event_id for de-dup; fall back to ts if absent.
            let dedup_id = if event_id.is_empty() {
                format!("slack:{channel}:{ts}")
            } else {
                event_id.clone()
            };
            Ok(ImEvent::Message(IncomingMessage {
                provider: "slack".into(),
                user_external_id: user,
                tenant_external_id: team_id,
                conversation_id: channel,
                text,
                event_id: dedup_id,
            }))
        }
        "app_home_opened" => {
            // App Home tab opened — informational, no text payload.
            let user = event
                .user
                .ok_or_else(|| ProviderError::Malformed("app_home_opened missing user".into()))?;
            let tab = event.tab.unwrap_or_else(|| "home".into());
            Ok(ImEvent::Message(IncomingMessage {
                provider: "slack".into(),
                user_external_id: user.clone(),
                tenant_external_id: team_id,
                // App Home is a direct-message-like surface; use user id
                // as the channel so the reply path knows where to deliver.
                conversation_id: format!("home:{user}"),
                text: format!("[app_home_opened tab={tab}]"),
                event_id: if event_id.is_empty() {
                    format!("home:{user}")
                } else {
                    event_id
                },
            }))
        }
        "reaction_added" => {
            let user = event
                .user
                .ok_or_else(|| ProviderError::Malformed("reaction_added missing user".into()))?;
            let reaction = event.reaction.ok_or_else(|| {
                ProviderError::Malformed("reaction_added missing reaction".into())
            })?;
            let item = event.item.unwrap_or(ReactionItem {
                kind: "unknown".into(),
                channel: None,
                ts: None,
            });
            let channel = item.channel.clone().unwrap_or_default();
            let item_ts = item.ts.clone().unwrap_or_default();
            Ok(ImEvent::Message(IncomingMessage {
                provider: "slack".into(),
                user_external_id: user,
                tenant_external_id: team_id,
                conversation_id: channel,
                text: format!("[reaction_added :{reaction}: on {item_ts}]"),
                event_id: if event_id.is_empty() {
                    format!("reaction:{item_ts}")
                } else {
                    event_id
                },
            }))
        }
        other => Err(ProviderError::Malformed(format!(
            "unsupported event type: {other}"
        ))),
    }
}

// ── re-export the raw deserialized type for Socket Mode ──────────────────────

/// Minimal shape of a Slack Socket Mode `events_api` envelope so
/// [`crate::socket_mode`] can call back into this parser.
#[derive(Deserialize)]
pub struct SocketModePayload {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub payload: Option<JsonValue>,
    #[serde(default)]
    pub envelope_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── URL verification ──────────────────────────────────────────────────────

    #[test]
    fn url_verification_challenge_round_trip() {
        let body = serde_json::to_vec(&json!({
            "type": "url_verification",
            "challenge": "3eZbrw1aBm2rZgRNFdxV2595E9CY3gmdALWMmHkvFXO7tYXAYM8P"
        }))
        .unwrap();
        match parse_event(&body, None).unwrap() {
            ImEvent::Challenge { challenge } => {
                assert_eq!(
                    challenge,
                    "3eZbrw1aBm2rZgRNFdxV2595E9CY3gmdALWMmHkvFXO7tYXAYM8P"
                );
            }
            other => panic!("expected Challenge, got {other:?}"),
        }
    }

    #[test]
    fn url_verification_missing_challenge_is_malformed() {
        let body = serde_json::to_vec(&json!({"type": "url_verification"})).unwrap();
        assert!(matches!(
            parse_event(&body, None),
            Err(ProviderError::Malformed(_))
        ));
    }

    // ── message event ─────────────────────────────────────────────────────────

    #[test]
    fn message_event_parses_all_fields() {
        let body = serde_json::to_vec(&json!({
            "type": "event_callback",
            "team_id": "T123",
            "event_id": "Ev456",
            "event": {
                "type": "message",
                "channel": "C789",
                "user": "U001",
                "text": "hello bot",
                "ts": "1716355200.000100"
            }
        }))
        .unwrap();
        match parse_event(&body, None).unwrap() {
            ImEvent::Message(m) => {
                assert_eq!(m.provider, "slack");
                assert_eq!(m.user_external_id, "U001");
                assert_eq!(m.tenant_external_id, "T123");
                assert_eq!(m.conversation_id, "C789");
                assert_eq!(m.text, "hello bot");
                assert_eq!(m.event_id, "Ev456");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn bot_message_is_ignored_to_prevent_loops() {
        let body = serde_json::to_vec(&json!({
            "type": "event_callback",
            "team_id": "T123",
            "event_id": "Ev001",
            "event": {
                "type": "message",
                "channel": "C789",
                "bot_id": "B001",
                "text": "I am a bot"
            }
        }))
        .unwrap();
        assert!(parse_event(&body, None).unwrap().is_ignored());
    }

    // ── app_mention ───────────────────────────────────────────────────────────

    #[test]
    fn app_mention_event_parses_correctly() {
        let body = serde_json::to_vec(&json!({
            "type": "event_callback",
            "team_id": "T_WORKSPACE",
            "event_id": "Ev_MENTION",
            "event": {
                "type": "app_mention",
                "channel": "C_GENERAL",
                "user": "U_ALICE",
                "text": "<@U_BOT> hello",
                "ts": "1716355300.000200"
            }
        }))
        .unwrap();
        match parse_event(&body, None).unwrap() {
            ImEvent::Message(m) => {
                assert_eq!(m.user_external_id, "U_ALICE");
                assert_eq!(m.conversation_id, "C_GENERAL");
                assert_eq!(m.text, "<@U_BOT> hello");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ── app_home_opened ───────────────────────────────────────────────────────

    #[test]
    fn app_home_opened_produces_synthetic_message() {
        let body = serde_json::to_vec(&json!({
            "type": "event_callback",
            "team_id": "T_HOME",
            "event_id": "Ev_HOME",
            "event": {
                "type": "app_home_opened",
                "user": "U_BOB",
                "tab": "home"
            }
        }))
        .unwrap();
        match parse_event(&body, None).unwrap() {
            ImEvent::Message(m) => {
                assert_eq!(m.user_external_id, "U_BOB");
                assert!(m.conversation_id.starts_with("home:"));
                assert!(m.text.contains("app_home_opened"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ── reaction_added ────────────────────────────────────────────────────────

    #[test]
    fn reaction_added_event_parses_reaction_name_and_item() {
        let body = serde_json::to_vec(&json!({
            "type": "event_callback",
            "team_id": "T_REACT",
            "event_id": "Ev_REACT",
            "event": {
                "type": "reaction_added",
                "user": "U_CAROL",
                "reaction": "thumbsup",
                "item": {
                    "type": "message",
                    "channel": "C_REACT",
                    "ts": "1716355400.000300"
                }
            }
        }))
        .unwrap();
        match parse_event(&body, None).unwrap() {
            ImEvent::Message(m) => {
                assert_eq!(m.user_external_id, "U_CAROL");
                assert_eq!(m.conversation_id, "C_REACT");
                assert!(m.text.contains("thumbsup"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ── retry handling ────────────────────────────────────────────────────────

    #[test]
    fn slack_retry_header_nonzero_is_ignored() {
        let body = serde_json::to_vec(&json!({
            "type": "event_callback",
            "team_id": "T123",
            "event_id": "Ev_RETRY",
            "event": {
                "type": "message",
                "channel": "C789",
                "user": "U001",
                "text": "hello"
            }
        }))
        .unwrap();
        // First delivery (retry_num = "0") must pass through.
        assert!(!parse_event(&body, Some("0")).unwrap().is_ignored());
        // Second delivery (retry_num = "1") must be silently dropped.
        assert!(parse_event(&body, Some("1")).unwrap().is_ignored());
    }

    // ── unknown types ─────────────────────────────────────────────────────────

    #[test]
    fn unknown_envelope_type_is_malformed() {
        let body = serde_json::to_vec(&json!({"type": "block_actions"})).unwrap();
        assert!(matches!(
            parse_event(&body, None),
            Err(ProviderError::Malformed(_))
        ));
    }

    #[test]
    fn unknown_event_type_in_callback_is_malformed() {
        let body = serde_json::to_vec(&json!({
            "type": "event_callback",
            "team_id": "T1",
            "event": {"type": "workflow_step_execute"}
        }))
        .unwrap();
        assert!(matches!(
            parse_event(&body, None),
            Err(ProviderError::Malformed(_))
        ));
    }
}
