//! Webhook path for the Telegram Bot API adapter.
//!
//! Telegram delivers `Update` objects via HTTPS POST to a URL registered with
//! `setWebhook`. When a `secret_token` is configured on the webhook, Telegram
//! includes it verbatim in the `X-Telegram-Bot-Api-Secret-Token` header so
//! the receiving server can reject spoofed deliveries.
//!
//! Security note: we use a constant-time comparison so timing side-channels
//! cannot leak a partial prefix of the configured secret.
//!
//! After verification the body is decoded as a Telegram `Update` and mapped to
//! one of:
//!   - `ImEvent::Message` — new text message or bot command (`/start`, …)
//!   - `ImEvent::Message` (edited) — `edited_message` update
//!   - `ImEvent::Message` (callback) — `callback_query` button press

use serde_json::Value as JsonValue;
use xiaoguai_im_gateway::{ImEvent, IncomingMessage, ProviderError, Webhook};

use crate::types::{CallbackQuery, Message, Update};

/// Constant-time byte-slice equality. Avoids early-exit timing leak.
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Verify the `X-Telegram-Bot-Api-Secret-Token` header against the configured
/// `secret_token`. Returns `Ok(())` when the token matches.
///
/// If `secret_token` is `None`, the check is skipped entirely — useful for
/// development without a registered webhook.
pub fn verify_secret(webhook: &Webhook, secret_token: Option<&str>) -> Result<(), ProviderError> {
    let Some(expected) = secret_token else {
        return Ok(());
    };
    let received = webhook
        .header("X-Telegram-Bot-Api-Secret-Token")
        .ok_or(ProviderError::BadSignature)?;
    if constant_time_eq(expected.as_bytes(), received.as_bytes()) {
        Ok(())
    } else {
        Err(ProviderError::BadSignature)
    }
}

/// Decode an `Update` payload and convert it to an [`ImEvent`].
///
/// Mapping rules:
/// - `message` with text → `ImEvent::Message` (text or bot command)
/// - `edited_message` with text → `ImEvent::Message` (`event_id` prefixed with `edited_`)
/// - `callback_query` → `ImEvent::Message` (`event_id` = query id, text = callback data)
/// - Anything else → `ProviderError::Malformed`
pub fn parse_update(body: &[u8]) -> Result<ImEvent, ProviderError> {
    let update: Update = serde_json::from_slice(body)
        .map_err(|e| ProviderError::Malformed(format!("decode update: {e}")))?;

    if let Some(msg) = update.message {
        return Ok(message_event("telegram", update.update_id, &msg, false));
    }

    if let Some(msg) = update.edited_message {
        return Ok(message_event("telegram", update.update_id, &msg, true));
    }

    if let Some(cbq) = update.callback_query {
        return Ok(callback_event(&cbq));
    }

    Err(ProviderError::Malformed(
        "update contains no recognised payload (message/edited_message/callback_query)".into(),
    ))
}

fn message_event(provider: &str, update_id: i64, msg: &Message, edited: bool) -> ImEvent {
    let user_id = msg
        .from
        .as_ref()
        .map(|u| u.id.to_string())
        .unwrap_or_default();
    let chat_id = msg.chat.id.to_string();
    let text = msg.text.clone().unwrap_or_default();
    let prefix = if edited { "edited_" } else { "" };
    let event_id = format!("{prefix}{update_id}_{}", msg.message_id);

    ImEvent::Message(IncomingMessage {
        provider: provider.into(),
        user_external_id: user_id,
        // Telegram has no tenant concept; we use the chat id as a stable
        // per-conversation namespace.
        tenant_external_id: chat_id.clone(),
        conversation_id: chat_id,
        text,
        event_id,
    })
}

fn callback_event(cbq: &CallbackQuery) -> ImEvent {
    let chat_id = cbq
        .message
        .as_ref()
        .map(|m| m.chat.id.to_string())
        .unwrap_or_default();
    let text = cbq.data.clone().unwrap_or_default();

    ImEvent::Message(IncomingMessage {
        provider: "telegram".into(),
        user_external_id: cbq.from.id.to_string(),
        tenant_external_id: chat_id.clone(),
        conversation_id: chat_id,
        text,
        event_id: cbq.id.clone(),
    })
}

// ---------------------------------------------------------------------------
// Outgoing reply JSON builder (used by outbound.rs to build the send payload)
// ---------------------------------------------------------------------------

/// Parse-mode values Telegram accepts in `sendMessage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseMode {
    Markdown,
    MarkdownV2,
    Html,
}

impl ParseMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ParseMode::Markdown => "Markdown",
            ParseMode::MarkdownV2 => "MarkdownV2",
            ParseMode::Html => "HTML",
        }
    }
}

/// Build the JSON body for a `sendMessage` request.
pub fn send_message_body(chat_id: &str, text: &str, parse_mode: Option<ParseMode>) -> JsonValue {
    let mut body = serde_json::json!({
        "chat_id": chat_id,
        "text": text,
    });
    if let Some(pm) = parse_mode {
        body["parse_mode"] = serde_json::json!(pm.as_str());
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // Secret-token verification
    // ------------------------------------------------------------------

    fn webhook_with_token(token: &str) -> Webhook {
        Webhook {
            headers: vec![("X-Telegram-Bot-Api-Secret-Token".into(), token.into())],
            body: b"{}".to_vec(),
        }
    }

    #[test]
    fn verify_happy_path() {
        let wh = webhook_with_token("my-secret");
        assert!(verify_secret(&wh, Some("my-secret")).is_ok());
    }

    #[test]
    fn verify_mismatch_rejected() {
        let wh = webhook_with_token("wrong");
        assert!(matches!(
            verify_secret(&wh, Some("my-secret")),
            Err(ProviderError::BadSignature)
        ));
    }

    #[test]
    fn verify_missing_header_rejected() {
        let wh = Webhook {
            headers: vec![],
            body: b"{}".to_vec(),
        };
        assert!(matches!(
            verify_secret(&wh, Some("my-secret")),
            Err(ProviderError::BadSignature)
        ));
    }

    #[test]
    fn verify_skipped_when_no_secret_configured() {
        let wh = Webhook {
            headers: vec![],
            body: b"{}".to_vec(),
        };
        assert!(verify_secret(&wh, None).is_ok());
    }

    // ------------------------------------------------------------------
    // Update payload parsing — text message
    // ------------------------------------------------------------------

    fn text_message_body(update_id: i64, text: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "update_id": update_id,
            "message": {
                "message_id": 101,
                "from": {"id": 42, "first_name": "Alice", "username": "alice"},
                "chat": {"id": 99, "type": "private"},
                "text": text
            }
        }))
        .unwrap()
    }

    #[test]
    fn parse_text_message() {
        let body = text_message_body(1, "hello");
        match parse_update(&body).unwrap() {
            ImEvent::Message(m) => {
                assert_eq!(m.user_external_id, "42");
                assert_eq!(m.conversation_id, "99");
                assert_eq!(m.text, "hello");
                assert_eq!(m.event_id, "1_101");
                assert_eq!(m.provider, "telegram");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_bot_command_start() {
        let body = text_message_body(2, "/start");
        match parse_update(&body).unwrap() {
            ImEvent::Message(m) => {
                assert_eq!(m.text, "/start");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ------------------------------------------------------------------
    // edited_message
    // ------------------------------------------------------------------

    #[test]
    fn parse_edited_message() {
        let body = serde_json::to_vec(&serde_json::json!({
            "update_id": 5,
            "edited_message": {
                "message_id": 200,
                "from": {"id": 7, "first_name": "Bob"},
                "chat": {"id": 55, "type": "group"},
                "text": "corrected"
            }
        }))
        .unwrap();
        match parse_update(&body).unwrap() {
            ImEvent::Message(m) => {
                assert_eq!(m.event_id, "edited_5_200");
                assert_eq!(m.text, "corrected");
                assert_eq!(m.user_external_id, "7");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ------------------------------------------------------------------
    // callback_query
    // ------------------------------------------------------------------

    #[test]
    fn parse_callback_query() {
        let body = serde_json::to_vec(&serde_json::json!({
            "update_id": 8,
            "callback_query": {
                "id": "cbq-xyz",
                "from": {"id": 10, "first_name": "Carol"},
                "data": "button_pressed",
                "message": {
                    "message_id": 300,
                    "chat": {"id": 77, "type": "private"},
                    "text": "choose"
                }
            }
        }))
        .unwrap();
        match parse_update(&body).unwrap() {
            ImEvent::Message(m) => {
                assert_eq!(m.event_id, "cbq-xyz");
                assert_eq!(m.text, "button_pressed");
                assert_eq!(m.user_external_id, "10");
                assert_eq!(m.conversation_id, "77");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ------------------------------------------------------------------
    // parse_mode round-trip via send_message_body
    // ------------------------------------------------------------------

    #[test]
    fn send_message_body_no_parse_mode() {
        let body = send_message_body("99", "hi", None);
        assert_eq!(body["chat_id"], "99");
        assert_eq!(body["text"], "hi");
        assert!(body.get("parse_mode").is_none());
    }

    #[test]
    fn send_message_body_markdown() {
        let body = send_message_body("1", "**bold**", Some(ParseMode::Markdown));
        assert_eq!(body["parse_mode"], "Markdown");
    }

    #[test]
    fn send_message_body_html() {
        let body = send_message_body("1", "<b>bold</b>", Some(ParseMode::Html));
        assert_eq!(body["parse_mode"], "HTML");
    }

    #[test]
    fn send_message_body_markdownv2() {
        let body = send_message_body("1", r"\_escaped\_", Some(ParseMode::MarkdownV2));
        assert_eq!(body["parse_mode"], "MarkdownV2");
    }

    // ------------------------------------------------------------------
    // constant_time_eq property
    // ------------------------------------------------------------------

    #[test]
    fn constant_time_eq_same() {
        assert!(constant_time_eq(b"hello", b"hello"));
    }

    #[test]
    fn constant_time_eq_diff_content() {
        assert!(!constant_time_eq(b"hello", b"world"));
    }

    #[test]
    fn constant_time_eq_diff_length() {
        assert!(!constant_time_eq(b"hi", b"hix"));
    }
}
