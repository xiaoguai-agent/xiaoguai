//! Telegram push sink — `sendMessage` against the public Bot API.
//!
//! Endpoint shape (the official `bots/api#sendmessage` contract):
//!
//! ```text
//! POST https://api.telegram.org/bot<token>/sendMessage
//! { "chat_id": "<chat_id>", "text": "<text>" }
//! ```
//!
//! The base URL is configurable so the mockito test can point it at a
//! local server; production wiring leaves it at the default.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::sink::{PushPayload, PushSink, SinkError};

pub const DEFAULT_TELEGRAM_BASE_URL: &str = "https://api.telegram.org";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TelegramSinkConfig {
    pub bot_token: String,
    /// Telegram `chat_id` (numeric or `@channelname`).
    pub chat_id: String,
    /// Override the Telegram base URL. Defaults to
    /// [`DEFAULT_TELEGRAM_BASE_URL`] when unset.
    #[serde(default)]
    pub base_url: Option<String>,
}

pub struct TelegramPushSink {
    id: String,
    client: reqwest::Client,
    base_url: String,
    bot_token: String,
    chat_id: String,
}

impl TelegramPushSink {
    /// Build a sink. Returns an error only if the underlying reqwest
    /// client can't be constructed (e.g. TLS init failure).
    ///
    /// # Errors
    /// Bubbles up the reqwest builder error.
    pub fn new(id: impl Into<String>, cfg: TelegramSinkConfig) -> Result<Self, SinkError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| SinkError::Delivery(format!("build reqwest: {e}")))?;
        Ok(Self {
            id: id.into(),
            client,
            base_url: cfg
                .base_url
                .unwrap_or_else(|| DEFAULT_TELEGRAM_BASE_URL.to_string()),
            bot_token: cfg.bot_token,
            chat_id: cfg.chat_id,
        })
    }

    /// Public so tests can assert on the rendered text without
    /// touching the HTTP layer.
    #[must_use]
    pub fn render_text(payload: &PushPayload) -> String {
        use std::fmt::Write as _;
        let mut buf = String::new();
        if payload.is_proactive && !payload.reason.is_empty() {
            buf.push_str("🔔 ");
            buf.push_str(&payload.reason);
            buf.push_str("\n\n");
        }
        let _ = write!(
            buf,
            "Job {} #{} [{}]",
            payload.job_id, payload.run_id, payload.status
        );
        if let Some(out) = &payload.output_preview {
            buf.push_str("\n\n");
            buf.push_str(out);
        }
        if let Some(err) = &payload.error_message {
            buf.push_str("\n\nError: ");
            buf.push_str(err);
        }
        buf
    }
}

impl std::fmt::Debug for TelegramPushSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramPushSink")
            .field("id", &self.id)
            .field("chat_id", &self.chat_id)
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl PushSink for TelegramPushSink {
    fn id(&self) -> &str {
        &self.id
    }

    async fn deliver(&self, payload: &PushPayload) -> Result<(), SinkError> {
        payload.require_reason_when_proactive()?;
        let url = format!("{}/bot{}/sendMessage", self.base_url, self.bot_token);
        let body = serde_json::json!({
            "chat_id": self.chat_id,
            "text": Self::render_text(payload),
        });
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| SinkError::Delivery(format!("send: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SinkError::Delivery(format!(
                "telegram http {status}: {body}"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn payload(is_proactive: bool, reason: &str) -> PushPayload {
        PushPayload {
            job_id: "j1".into(),
            run_id: 9,
            tenant_id: None,
            status: "succeeded".into(),
            fired_at: Utc::now(),
            output_preview: Some("output".into()),
            error_message: None,
            reason: reason.into(),
            is_proactive,
        }
    }

    #[tokio::test]
    async fn proactive_without_reason_is_refused_no_http() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", mockito::Matcher::Any)
            .expect(0)
            .create_async()
            .await;
        let sink = TelegramPushSink::new(
            "tg",
            TelegramSinkConfig {
                bot_token: "BOT".into(),
                chat_id: "12345".into(),
                base_url: Some(server.url()),
            },
        )
        .unwrap();
        let err = sink.deliver(&payload(true, "")).await.unwrap_err();
        assert!(matches!(err, SinkError::Invalid(_)));
        m.assert_async().await;
    }

    #[tokio::test]
    async fn scheduled_payload_posts_to_sendmessage_endpoint() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/botBOT/sendMessage")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "chat_id": "12345"
            })))
            .with_status(200)
            .with_body(r#"{"ok":true}"#)
            .create_async()
            .await;
        let sink = TelegramPushSink::new(
            "tg",
            TelegramSinkConfig {
                bot_token: "BOT".into(),
                chat_id: "12345".into(),
                base_url: Some(server.url()),
            },
        )
        .unwrap();
        sink.deliver(&payload(false, "")).await.unwrap();
        m.assert_async().await;
    }

    #[tokio::test]
    async fn proactive_with_reason_includes_bell_and_reason() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/botBOT/sendMessage")
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"text":"🔔 daily HN scan\n\nJob j1 #9 [succeeded]\n\noutput"}"#.into(),
            ))
            .with_status(200)
            .with_body(r#"{"ok":true}"#)
            .create_async()
            .await;
        let sink = TelegramPushSink::new(
            "tg",
            TelegramSinkConfig {
                bot_token: "BOT".into(),
                chat_id: "12345".into(),
                base_url: Some(server.url()),
            },
        )
        .unwrap();
        sink.deliver(&payload(true, "daily HN scan")).await.unwrap();
        m.assert_async().await;
    }

    #[tokio::test]
    async fn telegram_http_error_propagates_as_delivery() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/botBOT/sendMessage")
            .with_status(401)
            .with_body("Unauthorized")
            .create_async()
            .await;
        let sink = TelegramPushSink::new(
            "tg",
            TelegramSinkConfig {
                bot_token: "BOT".into(),
                chat_id: "12345".into(),
                base_url: Some(server.url()),
            },
        )
        .unwrap();
        let err = sink.deliver(&payload(false, "")).await.unwrap_err();
        assert!(matches!(err, SinkError::Delivery(msg) if msg.contains("401")));
    }
}
