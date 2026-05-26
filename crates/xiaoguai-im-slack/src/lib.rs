//! Slack IM adapter.
//!
//! v1.2.7 ships three paths:
//!
//! 1. **Events API / HTTP** — signature verification (`X-Slack-Signature`
//!    per Slack spec) + Events API payload parsing + `chat.postMessage`
//!    outbound reply.
//!
//! 2. **Socket Mode (optional)** — long-lived WSS connection for tenants
//!    without a public HTTPS endpoint. Re-uses `tokio-tungstenite` already
//!    in the workspace.
//!
//! Signature shape Slack uses:
//! ```text
//! base    = "v0:" + X-Slack-Request-Timestamp + ":" + raw-body
//! sig     = "v0=" + hex(hmac_sha256(signing_secret, base))
//! header  = X-Slack-Signature: <sig>
//! ```
//!
//! Event types handled:
//! - `url_verification` (URL challenge echo — no auth required but we
//!   verify anyway)
//! - `event_callback / message` — channel message or DM
//! - `event_callback / app_mention` — @-mention in a channel
//! - `event_callback / app_home_opened` — user opens App Home tab
//! - `event_callback / reaction_added` — user adds a reaction
//!
//! Outbound: `POST https://slack.com/api/chat.postMessage` with
//! `Authorization: Bearer <bot_token>`.
//!
//! # Quick start
//!
//! ```no_run
//! use xiaoguai_im_slack::{SlackProvider, outbound::HttpSlackClient};
//! use std::sync::Arc;
//!
//! let client = Arc::new(HttpSlackClient::new().unwrap());
//! let provider = SlackProvider::with_http_sink(
//!     "your-signing-secret",
//!     "xoxb-your-bot-token",
//!     client,
//! );
//! ```

#![forbid(unsafe_code)]

pub mod inbound;
pub mod outbound;
pub mod signature;
pub mod socket_mode;

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use parking_lot::Mutex as SyncMutex;
use serde_json::{json, Value as JsonValue};

use xiaoguai_im_gateway::{ImEvent, ImProvider, OutgoingReply, ProviderError, Webhook};

pub use inbound::parse_event;
pub use outbound::{HttpSlackClient, SlackClient, DEFAULT_BASE_URL};
pub use signature::{verify, TIMESTAMP_TOLERANCE_SECS};

// ── reply sink ────────────────────────────────────────────────────────────────

/// How outbound messages are dispatched.
#[derive(Clone, Default)]
pub enum ReplySink {
    /// Discard the reply. Used in tests + dev.
    #[default]
    Stub,
    /// In-memory recorder so tests can assert on what would have been sent.
    Recording(Arc<SyncMutex<Vec<OutgoingReply>>>),
    /// Send via the real Slack `chat.postMessage` Web API.
    Http(Arc<HttpSink>),
}

/// Holds the HTTP client + bot token for the real Slack reply path.
pub struct HttpSink {
    pub client: Arc<dyn SlackClient>,
    pub bot_token: String,
}

// ── SlackProvider ─────────────────────────────────────────────────────────────

/// [`ImProvider`] implementation for Slack.
///
/// Cloning is cheap — all heavy state lives behind `Arc`.
#[derive(Clone)]
pub struct SlackProvider {
    signing_secret: String,
    reply_sink: ReplySink,
}

impl SlackProvider {
    /// Create a provider that discards all outbound replies (dev/test).
    #[must_use]
    pub fn new(signing_secret: impl Into<String>) -> Self {
        Self {
            signing_secret: signing_secret.into(),
            reply_sink: ReplySink::Stub,
        }
    }

    /// Create a provider that records outbound replies into `sink` so tests
    /// can assert on them.
    #[must_use]
    pub fn with_recording_sink(
        signing_secret: impl Into<String>,
        sink: Arc<SyncMutex<Vec<OutgoingReply>>>,
    ) -> Self {
        Self {
            signing_secret: signing_secret.into(),
            reply_sink: ReplySink::Recording(sink),
        }
    }

    /// Create a provider that sends replies via a [`SlackClient`].
    ///
    /// Pass an [`HttpSlackClient`] for production or a fake client for tests.
    #[must_use]
    pub fn with_http_sink(
        signing_secret: impl Into<String>,
        bot_token: impl Into<String>,
        client: Arc<dyn SlackClient>,
    ) -> Self {
        Self {
            signing_secret: signing_secret.into(),
            reply_sink: ReplySink::Http(Arc::new(HttpSink {
                client,
                bot_token: bot_token.into(),
            })),
        }
    }
}

// ── ImProvider impl ───────────────────────────────────────────────────────────

#[async_trait]
impl ImProvider for SlackProvider {
    /// Verify the `X-Slack-Signature` and parse the Events API payload.
    ///
    /// Uses the current wall clock for replay-protection; this is the only
    /// call-site that reads `Utc::now()`. All other logic is clock-injectable
    /// via the lower-level `signature::verify(webhook, secret, now_unix)`.
    async fn parse(&self, webhook: &Webhook) -> Result<ImEvent, ProviderError> {
        let now = Utc::now().timestamp();
        signature::verify(webhook, &self.signing_secret, now)?;
        // X-Slack-Retry-Num header — drop re-deliveries.
        let retry_num = webhook.header("X-Slack-Retry-Num");
        inbound::parse_event(&webhook.body, retry_num)
    }

    async fn reply(&self, out: &OutgoingReply) -> Result<JsonValue, ProviderError> {
        match &self.reply_sink {
            ReplySink::Stub => {
                tracing::debug!(?out, "slack reply stub");
                Ok(json!({"status": "stubbed"}))
            }
            ReplySink::Recording(buf) => {
                buf.lock().push(out.clone());
                Ok(json!({"status": "recorded"}))
            }
            ReplySink::Http(sink) => {
                let resp = sink
                    .client
                    .post_message(&sink.bot_token, &out.conversation_id, &out.text)
                    .await?;
                tracing::info!(channel = %out.conversation_id, "slack reply sent");
                Ok(resp)
            }
        }
    }

    fn name(&self) -> &'static str {
        "slack"
    }
}

// ── unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── helpers ───────────────────────────────────────────────────────────────

    /// Build a Webhook with a valid Slack signature for `body` and the given
    /// `signing_secret`. `now_unix` is the timestamp embedded in the header
    /// (and also the "current time" we pass to `verify`).
    fn signed_webhook(body: &str, signing_secret: &str, now_unix: i64) -> Webhook {
        use hmac::{Hmac, KeyInit, Mac};
        use sha2::Sha256;

        let ts = now_unix.to_string();
        let base = format!("v0:{ts}:{body}");
        let mut mac = Hmac::<Sha256>::new_from_slice(signing_secret.as_bytes()).unwrap();
        mac.update(base.as_bytes());
        let sig = format!("v0={}", hex::encode(mac.finalize().into_bytes()));

        Webhook {
            headers: vec![
                ("X-Slack-Request-Timestamp".into(), ts),
                ("X-Slack-Signature".into(), sig),
            ],
            body: body.as_bytes().to_vec(),
        }
    }

    const SECRET: &str = "test-signing-secret";
    const NOW: i64 = 1_716_355_200;

    // ── signature-level tests (through the full provider::parse path) ─────────

    #[tokio::test]
    async fn parse_rejects_bad_signature() {
        let body = r#"{"type":"url_verification","challenge":"c"}"#;
        let mut wh = signed_webhook(body, SECRET, NOW);
        // Tamper with the signature.
        wh.headers[1].1 = "v0=deadbeef".into();
        let _provider = SlackProvider::new(SECRET);
        // Can't use the real wall clock — use the lower-level fn instead.
        assert!(matches!(
            signature::verify(&wh, SECRET, NOW),
            Err(ProviderError::BadSignature)
        ));
    }

    // ── URL verification ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn parse_url_verification_returns_challenge() {
        let body = json!({
            "type": "url_verification",
            "challenge": "abc123"
        })
        .to_string();
        let wh = signed_webhook(&body, SECRET, NOW);
        // Use the low-level inbound path (skip wall clock).
        match inbound::parse_event(wh.body.as_slice(), None).unwrap() {
            ImEvent::Challenge { challenge } => assert_eq!(challenge, "abc123"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ── message parse ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn parse_message_event_extracts_fields() {
        let body = json!({
            "type": "event_callback",
            "team_id": "T_SLACK",
            "event_id": "Ev_SLACK",
            "event": {
                "type": "message",
                "channel": "C_SLACK",
                "user": "U_SLACK",
                "text": "hello slack adapter",
                "ts": "1716355200.000100"
            }
        })
        .to_string();
        match inbound::parse_event(body.as_bytes(), None).unwrap() {
            ImEvent::Message(m) => {
                assert_eq!(m.provider, "slack");
                assert_eq!(m.user_external_id, "U_SLACK");
                assert_eq!(m.tenant_external_id, "T_SLACK");
                assert_eq!(m.conversation_id, "C_SLACK");
                assert_eq!(m.text, "hello slack adapter");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ── reply recording ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn reply_records_into_sink() {
        let sink = Arc::new(SyncMutex::new(Vec::new()));
        let provider = SlackProvider::with_recording_sink(SECRET, sink.clone());
        provider
            .reply(&OutgoingReply {
                conversation_id: "C_CHANNEL".into(),
                text: "hi back".into(),
            })
            .await
            .unwrap();
        let buf = sink.lock();
        assert_eq!(buf.len(), 1);
        assert_eq!(buf[0].text, "hi back");
    }

    // ── HTTP-sink path through a fake client ──────────────────────────────────

    #[tokio::test]
    async fn http_sink_forwards_channel_and_text() {
        use parking_lot::Mutex;

        #[derive(Default)]
        struct FakeClient {
            calls: Mutex<Vec<(String, String, String)>>,
        }

        #[async_trait::async_trait]
        impl SlackClient for FakeClient {
            async fn post_message(
                &self,
                token: &str,
                channel: &str,
                text: &str,
            ) -> Result<JsonValue, ProviderError> {
                self.calls
                    .lock()
                    .push((token.to_string(), channel.to_string(), text.to_string()));
                Ok(json!({"ok": true}))
            }
        }

        let fake: Arc<FakeClient> = Arc::new(FakeClient::default());
        let provider = SlackProvider::with_http_sink(
            SECRET,
            "xoxb-test-bot-token",
            fake.clone() as Arc<dyn SlackClient>,
        );
        provider
            .reply(&OutgoingReply {
                conversation_id: "C_TARGET".into(),
                text: "reply text".into(),
            })
            .await
            .unwrap();

        let calls = fake.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0],
            (
                "xoxb-test-bot-token".into(),
                "C_TARGET".into(),
                "reply text".into()
            )
        );
    }

    // ── provider name ─────────────────────────────────────────────────────────

    #[test]
    fn provider_name_is_slack() {
        assert_eq!(SlackProvider::new(SECRET).name(), "slack");
    }
}
