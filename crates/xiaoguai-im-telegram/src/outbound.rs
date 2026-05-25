//! Outbound Telegram Bot API calls.
//!
//! All calls go to `https://api.telegram.org/bot{token}/{method}`.
//!
//! The HTTP transport sits behind the [`TelegramClient`] trait so tests can
//! inject a fake without spinning up a real server (or using mockito for every
//! call site).
//!
//! Covered methods:
//! - `sendMessage` — send/reply with text
//! - `sendChatAction` — show "typing…" indicator
//! - `editMessageText` — update a previously-sent message
//! - `answerCallbackQuery` — acknowledge an inline-button press

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use tracing::instrument;
use xiaoguai_im_gateway::ProviderError;

use crate::webhook::ParseMode;

/// Default Telegram Bot API base URL.
pub const DEFAULT_BASE_URL: &str = "https://api.telegram.org";

/// Trait covering the minimum HTTP surface we need from the Telegram API.
///
/// Production wires this to [`HttpTelegramClient`]. Tests pass a fake.
#[async_trait]
pub trait TelegramClient: Send + Sync {
    /// Call any Bot API method with a JSON body.
    ///
    /// Returns the raw `result` value from the API response envelope
    /// `{"ok": true, "result": …}`.
    async fn call(&self, method: &str, body: JsonValue) -> Result<JsonValue, ProviderError>;
}

// ---------------------------------------------------------------------------
// Concrete reqwest-backed implementation
// ---------------------------------------------------------------------------

/// Reqwest-backed [`TelegramClient`].
pub struct HttpTelegramClient {
    client: reqwest::Client,
    base_url: String,
    bot_token: String,
}

impl HttpTelegramClient {
    /// Create a client pointed at the default `api.telegram.org`.
    ///
    /// # Errors
    /// Returns `ProviderError::Transport` if the `reqwest::Client` cannot
    /// be built (TLS initialisation failure on the host).
    pub fn new(bot_token: impl Into<String>) -> Result<Self, ProviderError> {
        Self::with_base_url(bot_token, DEFAULT_BASE_URL.to_string())
    }

    /// Create a client pointed at a custom base URL — useful for tests.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_base_url(
        bot_token: impl Into<String>,
        base_url: String,
    ) -> Result<Self, ProviderError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| ProviderError::Transport(format!("build reqwest: {e}")))?;
        Ok(Self {
            client,
            base_url,
            bot_token: bot_token.into(),
        })
    }
}

#[async_trait]
impl TelegramClient for HttpTelegramClient {
    #[instrument(skip(self, body), fields(method))]
    async fn call(&self, method: &str, body: JsonValue) -> Result<JsonValue, ProviderError> {
        let url = format!("{}/bot{}/{}", self.base_url, self.bot_token, method);
        let raw = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(format!("{method} send: {e}")))?
            .json::<JsonValue>()
            .await
            .map_err(|e| ProviderError::Transport(format!("{method} decode: {e}")))?;

        if raw.get("ok").and_then(JsonValue::as_bool) == Some(true) {
            Ok(raw.get("result").cloned().unwrap_or(JsonValue::Null))
        } else {
            let desc = raw
                .get("description")
                .and_then(JsonValue::as_str)
                .unwrap_or("unknown error");
            Err(ProviderError::Transport(format!(
                "Telegram {method} failed: {desc}"
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// High-level helpers built on top of TelegramClient
// ---------------------------------------------------------------------------

/// Send a text message to `chat_id`.
///
/// `parse_mode` controls Markdown/HTML rendering. Pass `None` for plain text.
///
/// # Errors
///
/// Returns [`ProviderError`] if the Telegram API call fails.
pub async fn send_message(
    client: &dyn TelegramClient,
    chat_id: &str,
    text: &str,
    parse_mode: Option<ParseMode>,
) -> Result<JsonValue, ProviderError> {
    let mut body = json!({
        "chat_id": chat_id,
        "text": text,
    });
    if let Some(pm) = parse_mode {
        body["parse_mode"] = json!(pm.as_str());
    }
    client.call("sendMessage", body).await
}

/// Broadcast a typing indicator. Telegram displays it for ~5 s.
///
/// # Errors
///
/// Returns [`ProviderError`] if the Telegram API call fails.
pub async fn send_chat_action(
    client: &dyn TelegramClient,
    chat_id: &str,
) -> Result<JsonValue, ProviderError> {
    client
        .call(
            "sendChatAction",
            json!({"chat_id": chat_id, "action": "typing"}),
        )
        .await
}

/// Replace the text of a previously-sent message.
///
/// `message_id` must be the message's `message_id` integer.
///
/// # Errors
///
/// Returns [`ProviderError`] if the Telegram API call fails.
pub async fn edit_message_text(
    client: &dyn TelegramClient,
    chat_id: &str,
    message_id: i64,
    text: &str,
    parse_mode: Option<ParseMode>,
) -> Result<JsonValue, ProviderError> {
    let mut body = json!({
        "chat_id": chat_id,
        "message_id": message_id,
        "text": text,
    });
    if let Some(pm) = parse_mode {
        body["parse_mode"] = json!(pm.as_str());
    }
    client.call("editMessageText", body).await
}

/// Acknowledge an inline-keyboard button press (`callback_query`).
///
/// `text` is the optional toast shown to the user (max 200 chars, Telegram
/// silently truncates). Pass `None` for a silent ack.
///
/// # Errors
///
/// Returns [`ProviderError`] if the Telegram API call fails.
pub async fn answer_callback_query(
    client: &dyn TelegramClient,
    callback_query_id: &str,
    text: Option<&str>,
) -> Result<JsonValue, ProviderError> {
    let mut body = json!({"callback_query_id": callback_query_id});
    if let Some(t) = text {
        body["text"] = json!(t);
    }
    client.call("answerCallbackQuery", body).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::Arc;

    // ---------------------------------------------------------------------------
    // Test double: records every call, returns a configurable result.
    // ---------------------------------------------------------------------------

    struct FakeClient {
        calls: Mutex<Vec<(String, JsonValue)>>,
        result: Mutex<Result<JsonValue, String>>,
    }

    impl FakeClient {
        fn ok(value: JsonValue) -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                result: Mutex::new(Ok(value)),
            })
        }
        fn err(msg: &str) -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                result: Mutex::new(Err(msg.to_string())),
            })
        }
        fn recorded(&self) -> Vec<(String, JsonValue)> {
            self.calls.lock().clone()
        }
    }

    #[async_trait]
    impl TelegramClient for FakeClient {
        async fn call(&self, method: &str, body: JsonValue) -> Result<JsonValue, ProviderError> {
            self.calls.lock().push((method.to_string(), body));
            match &*self.result.lock() {
                Ok(v) => Ok(v.clone()),
                Err(e) => Err(ProviderError::Transport(e.clone())),
            }
        }
    }

    // ---------------------------------------------------------------------------
    // sendMessage
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn send_message_plain() {
        let client = FakeClient::ok(json!({"message_id": 1}));
        let result = send_message(&*client, "42", "hello", None).await;
        assert!(result.is_ok());
        let calls = client.recorded();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "sendMessage");
        assert_eq!(calls[0].1["chat_id"], "42");
        assert_eq!(calls[0].1["text"], "hello");
        assert!(calls[0].1.get("parse_mode").is_none());
    }

    #[tokio::test]
    async fn send_message_with_markdown_parse_mode() {
        let client = FakeClient::ok(json!({}));
        send_message(&*client, "1", "*bold*", Some(ParseMode::Markdown))
            .await
            .unwrap();
        let calls = client.recorded();
        assert_eq!(calls[0].1["parse_mode"], "Markdown");
    }

    #[tokio::test]
    async fn send_message_with_html_parse_mode() {
        let client = FakeClient::ok(json!({}));
        send_message(&*client, "1", "<b>bold</b>", Some(ParseMode::Html))
            .await
            .unwrap();
        assert_eq!(client.recorded()[0].1["parse_mode"], "HTML");
    }

    #[tokio::test]
    async fn send_message_propagates_error() {
        let client = FakeClient::err("Bad Request: chat not found");
        let err = send_message(&*client, "99", "hi", None).await.unwrap_err();
        assert!(matches!(err, ProviderError::Transport(_)));
    }

    // ---------------------------------------------------------------------------
    // sendChatAction
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn send_chat_action_sends_typing() {
        let client = FakeClient::ok(json!(true));
        send_chat_action(&*client, "55").await.unwrap();
        let calls = client.recorded();
        assert_eq!(calls[0].0, "sendChatAction");
        assert_eq!(calls[0].1["action"], "typing");
        assert_eq!(calls[0].1["chat_id"], "55");
    }

    // ---------------------------------------------------------------------------
    // editMessageText
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn edit_message_text_plain() {
        let client = FakeClient::ok(json!({}));
        edit_message_text(&*client, "10", 200, "updated text", None)
            .await
            .unwrap();
        let calls = client.recorded();
        assert_eq!(calls[0].0, "editMessageText");
        assert_eq!(calls[0].1["message_id"], 200);
        assert_eq!(calls[0].1["text"], "updated text");
        assert!(calls[0].1.get("parse_mode").is_none());
    }

    #[tokio::test]
    async fn edit_message_text_with_html() {
        let client = FakeClient::ok(json!({}));
        edit_message_text(&*client, "10", 5, "<i>hi</i>", Some(ParseMode::Html))
            .await
            .unwrap();
        assert_eq!(client.recorded()[0].1["parse_mode"], "HTML");
    }

    // ---------------------------------------------------------------------------
    // answerCallbackQuery
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn answer_callback_query_silent() {
        let client = FakeClient::ok(json!(true));
        answer_callback_query(&*client, "cbq-123", None)
            .await
            .unwrap();
        let calls = client.recorded();
        assert_eq!(calls[0].0, "answerCallbackQuery");
        assert_eq!(calls[0].1["callback_query_id"], "cbq-123");
        assert!(calls[0].1.get("text").is_none());
    }

    #[tokio::test]
    async fn answer_callback_query_with_toast() {
        let client = FakeClient::ok(json!(true));
        answer_callback_query(&*client, "cbq-999", Some("Done!"))
            .await
            .unwrap();
        assert_eq!(client.recorded()[0].1["text"], "Done!");
    }
}
