//! Telegram Bot API adapter.
//!
//! Implements [`ImProvider`] for the Telegram Bot API. Two receive modes are
//! supported:
//!
//! **Webhook** (recommended in production):
//! Register a URL with `setWebhook`; Telegram HTTPS-POSTs each `Update` to
//! that URL.  `TelegramProvider::parse` verifies the
//! `X-Telegram-Bot-Api-Secret-Token` header (constant-time compare) and
//! converts the payload into an [`ImEvent`].
//!
//! **Long-poll** (useful during development or behind NAT):
//! Call [`long_poll::fetch_updates`] / [`long_poll::run_long_poll`] in a
//! background task.  No public URL required.
//!
//! Outbound calls (`sendMessage`, `sendChatAction`, `editMessageText`,
//! `answerCallbackQuery`) live in [`outbound`] and are exposed behind the
//! [`TelegramClient`] trait so tests inject fakes.
//!
//! # Quick start
//! ```no_run
//! use xiaoguai_im_telegram::{TelegramProvider, outbound::HttpTelegramClient};
//! use std::sync::Arc;
//!
//! let client = Arc::new(HttpTelegramClient::new("BOT_TOKEN").unwrap());
//! let provider = TelegramProvider::with_client("BOT_TOKEN", Some("WEBHOOK_SECRET"), client);
//! ```

#![forbid(unsafe_code)]

pub mod long_poll;
pub mod outbound;
pub mod types;
pub mod webhook;

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{json, Value as JsonValue};
use xiaoguai_im_gateway::{ImEvent, ImProvider, OutgoingReply, ProviderError, Webhook};

pub use outbound::{
    answer_callback_query, edit_message_text, send_chat_action, send_message, HttpTelegramClient,
    TelegramClient, DEFAULT_BASE_URL,
};
pub use webhook::ParseMode;

// ---------------------------------------------------------------------------
// Reply sink — same pattern as xiaoguai-im-feishu's ReplySink
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
enum ReplySink {
    #[default]
    Stub,
    Recording(Arc<Mutex<Vec<OutgoingReply>>>),
    Api(Arc<dyn TelegramClient>),
}

// ---------------------------------------------------------------------------
// TelegramProvider
// ---------------------------------------------------------------------------

/// Telegram Bot API adapter that implements [`ImProvider`].
#[derive(Clone)]
pub struct TelegramProvider {
    /// Bot token (`123456:ABC-DEF…`).
    pub bot_token: String,
    /// Optional webhook secret for request verification.
    pub secret_token: Option<String>,
    reply_sink: ReplySink,
}

impl TelegramProvider {
    /// Create a provider that stubs all outbound calls (for tests / dev).
    #[must_use]
    pub fn new(bot_token: impl Into<String>, secret_token: Option<impl Into<String>>) -> Self {
        Self {
            bot_token: bot_token.into(),
            secret_token: secret_token.map(Into::into),
            reply_sink: ReplySink::Stub,
        }
    }

    /// Create a provider with an in-memory recording sink.
    ///
    /// Outbound replies are appended to `sink`; useful in tests where you
    /// want to assert on what would have been sent.
    #[must_use]
    pub fn with_recording_sink(
        bot_token: impl Into<String>,
        secret_token: Option<impl Into<String>>,
        sink: Arc<Mutex<Vec<OutgoingReply>>>,
    ) -> Self {
        Self {
            bot_token: bot_token.into(),
            secret_token: secret_token.map(Into::into),
            reply_sink: ReplySink::Recording(sink),
        }
    }

    /// Create a provider backed by a real (or fake) [`TelegramClient`].
    #[must_use]
    pub fn with_client(
        bot_token: impl Into<String>,
        secret_token: Option<impl Into<String>>,
        client: Arc<dyn TelegramClient>,
    ) -> Self {
        Self {
            bot_token: bot_token.into(),
            secret_token: secret_token.map(Into::into),
            reply_sink: ReplySink::Api(client),
        }
    }
}

#[async_trait]
impl ImProvider for TelegramProvider {
    async fn parse(&self, wh: &Webhook) -> Result<ImEvent, ProviderError> {
        webhook::verify_secret(wh, self.secret_token.as_deref())?;
        webhook::parse_update(&wh.body)
    }

    async fn reply(&self, out: &OutgoingReply) -> Result<JsonValue, ProviderError> {
        match &self.reply_sink {
            ReplySink::Stub => {
                tracing::debug!(?out, "telegram reply stub");
                Ok(json!({"status":"stubbed"}))
            }
            ReplySink::Recording(buf) => {
                buf.lock().push(out.clone());
                Ok(json!({"status":"recorded"}))
            }
            ReplySink::Api(client) => {
                let result =
                    send_message(client.as_ref(), &out.conversation_id, &out.text, None).await?;
                tracing::info!(chat_id = %out.conversation_id, "telegram reply sent");
                Ok(result)
            }
        }
    }

    fn name(&self) -> &'static str {
        "telegram"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_webhook(body: &str, secret: Option<&str>) -> Webhook {
        let mut headers = vec![];
        if let Some(s) = secret {
            headers.push(("X-Telegram-Bot-Api-Secret-Token".into(), s.into()));
        }
        Webhook {
            headers,
            body: body.as_bytes().to_vec(),
        }
    }

    fn text_update(update_id: i64, text: &str) -> String {
        serde_json::json!({
            "update_id": update_id,
            "message": {
                "message_id": 1,
                "from": {"id": 1, "first_name": "U"},
                "chat": {"id": 10, "type": "private"},
                "text": text
            }
        })
        .to_string()
    }

    // ------------------------------------------------------------------
    // parse: webhook verification gate
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn parse_rejects_wrong_secret() {
        let provider = TelegramProvider::new("tok", Some("correct"));
        let wh = make_webhook(&text_update(1, "hi"), Some("wrong"));
        assert!(matches!(
            provider.parse(&wh).await,
            Err(ProviderError::BadSignature)
        ));
    }

    #[tokio::test]
    async fn parse_accepts_correct_secret() {
        let provider = TelegramProvider::new("tok", Some("s3cr3t"));
        let wh = make_webhook(&text_update(1, "hi"), Some("s3cr3t"));
        assert!(provider.parse(&wh).await.is_ok());
    }

    #[tokio::test]
    async fn parse_skips_verify_when_no_secret() {
        let provider = TelegramProvider::new("tok", None::<String>);
        let wh = make_webhook(&text_update(1, "hi"), None);
        assert!(provider.parse(&wh).await.is_ok());
    }

    // ------------------------------------------------------------------
    // reply: recording sink
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn reply_records_into_sink() {
        let sink: Arc<Mutex<Vec<OutgoingReply>>> = Arc::new(Mutex::new(Vec::new()));
        let provider =
            TelegramProvider::with_recording_sink("tok", None::<String>, sink.clone());
        provider
            .reply(&OutgoingReply {
                conversation_id: "42".into(),
                text: "hello back".into(),
            })
            .await
            .unwrap();
        let buf = sink.lock();
        assert_eq!(buf.len(), 1);
        assert_eq!(buf[0].text, "hello back");
        assert_eq!(buf[0].conversation_id, "42");
    }

    // ------------------------------------------------------------------
    // reply: api sink (via fake TelegramClient)
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn reply_calls_through_to_client() {
        use parking_lot::Mutex as PLMutex;

        #[derive(Default)]
        struct Recorder {
            calls: PLMutex<Vec<(String, JsonValue)>>,
        }

        #[async_trait]
        impl TelegramClient for Recorder {
            async fn call(
                &self,
                method: &str,
                body: JsonValue,
            ) -> Result<JsonValue, ProviderError> {
                self.calls.lock().push((method.to_string(), body));
                Ok(json!({"message_id": 99}))
            }
        }

        let rec: Arc<Recorder> = Arc::new(Recorder::default());
        let provider = TelegramProvider::with_client(
            "tok",
            None::<String>,
            rec.clone() as Arc<dyn TelegramClient>,
        );
        provider
            .reply(&OutgoingReply {
                conversation_id: "77".into(),
                text: "world".into(),
            })
            .await
            .unwrap();
        let calls = rec.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "sendMessage");
        assert_eq!(calls[0].1["chat_id"], "77");
        assert_eq!(calls[0].1["text"], "world");
    }

    // ------------------------------------------------------------------
    // name()
    // ------------------------------------------------------------------

    #[test]
    fn name_is_telegram() {
        let p = TelegramProvider::new("tok", None::<String>);
        assert_eq!(p.name(), "telegram");
    }
}
