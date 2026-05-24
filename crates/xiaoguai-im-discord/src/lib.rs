//! Discord IM adapter for xiaoguai.
//!
//! # Overview
//!
//! `xiaoguai-im-discord` implements the [`ImProvider`] trait for Discord's
//! Interactions webhook model (slash commands + button clicks).
//!
//! ## Authentication
//!
//! Discord signs every Interactions request with an **Ed25519** signature.
//! The adapter verifies `X-Signature-Ed25519` + `X-Signature-Timestamp`
//! against the application's public key (available in the Discord Developer
//! Portal → General Information → Public Key).
//!
//! ## Interaction flow
//!
//! ```text
//! Discord → POST /v1/im/discord/webhook
//!   ├─ type 1 (PING)             → 200 { "type": 1 }  (PONG)
//!   ├─ type 2 (slash command)    → 200 accepted; agent runs async
//!   └─ type 3 (button click)     → 200 accepted; agent runs async
//! ```
//!
//! ## Reply sink
//!
//! | Variant | Purpose |
//! |---------|---------|
//! | `Stub`  | Discard reply (dev / tests) |
//! | `Recording` | Capture replies in memory (unit tests) |
//! | `Api`   | Send via `POST /api/v10/channels/{id}/messages` |
//!
//! ## Gateway WebSocket
//!
//! The [`gateway`] module contains the planned structure but is not yet
//! implemented. Slash-command bots don't need the Gateway.
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use xiaoguai_im_discord::{DiscordProvider, outbound::HttpDiscordClient};
//!
//! let client = Arc::new(HttpDiscordClient::new("Bot TOKEN").unwrap());
//! let provider = DiscordProvider::with_api_sink(
//!     "YOUR_APPLICATION_PUBLIC_KEY_HEX",
//!     client,
//!     "CHANNEL_ID_FOR_REPLIES",
//! ).unwrap();
//! // Mount via xiaoguai_im_gateway::mount_discord(app, Arc::new(provider))
//! ```

#![forbid(unsafe_code)]

pub mod gateway;
pub mod interactions;
pub mod outbound;
pub mod signature;

use std::sync::Arc;

use async_trait::async_trait;
use ed25519_dalek::VerifyingKey;
use serde_json::{json, Value as JsonValue};
use xiaoguai_im_gateway::{ImEvent, ImProvider, OutgoingReply, ProviderError, Webhook};

pub use interactions::{message_response, pong_response, Interaction};
pub use outbound::{DiscordClient, HttpDiscordClient, DEFAULT_BASE_URL};
pub use signature::parse_public_key;

// ── Reply sink ───────────────────────────────────────────────────────────────

/// How the provider delivers outbound replies.
#[derive(Clone, Default)]
pub enum ReplySink {
    /// Discard the reply. Used in tests and dev mode.
    #[default]
    Stub,
    /// Record replies in memory so tests can assert on them.
    Recording(Arc<parking_lot::Mutex<Vec<OutgoingReply>>>),
    /// Send via the Discord REST API.
    Api(Arc<ApiSink>),
}

pub struct ApiSink {
    pub client: Arc<dyn DiscordClient>,
    /// Default channel id used when the conversation id doesn't encode one.
    pub default_channel_id: String,
}

// ── Provider ─────────────────────────────────────────────────────────────────

/// Discord IM adapter.
///
/// Verified with Ed25519; handles PING (type 1), slash commands (type 2),
/// and button clicks (type 3).
#[derive(Clone)]
pub struct DiscordProvider {
    /// Ed25519 public key from the Discord Developer Portal.
    public_key: Arc<VerifyingKey>,
    reply_sink: ReplySink,
}

impl DiscordProvider {
    /// Create a stub provider (no outbound reply delivery) from a hex public key.
    ///
    /// # Errors
    /// Returns an error if `public_key_hex` is not a valid Ed25519 key.
    pub fn new(public_key_hex: &str) -> Result<Self, ProviderError> {
        let vk = parse_public_key(public_key_hex)?;
        Ok(Self {
            public_key: Arc::new(vk),
            reply_sink: ReplySink::Stub,
        })
    }

    /// Create a provider with a recording sink — replies are captured into
    /// `sink` rather than sent to Discord. Useful for unit tests.
    ///
    /// # Errors
    /// Returns an error if `public_key_hex` is not a valid Ed25519 key.
    pub fn with_recording_sink(
        public_key_hex: &str,
        sink: Arc<parking_lot::Mutex<Vec<OutgoingReply>>>,
    ) -> Result<Self, ProviderError> {
        let vk = parse_public_key(public_key_hex)?;
        Ok(Self {
            public_key: Arc::new(vk),
            reply_sink: ReplySink::Recording(sink),
        })
    }

    /// Create a provider that sends replies via the Discord REST API.
    ///
    /// `default_channel_id` is used when the conversation id does not
    /// encode an explicit Discord channel (e.g. DM conversations).
    ///
    /// # Errors
    /// Returns an error if `public_key_hex` is not a valid Ed25519 key.
    pub fn with_api_sink(
        public_key_hex: &str,
        client: Arc<dyn DiscordClient>,
        default_channel_id: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        let vk = parse_public_key(public_key_hex)?;
        Ok(Self {
            public_key: Arc::new(vk),
            reply_sink: ReplySink::Api(Arc::new(ApiSink {
                client,
                default_channel_id: default_channel_id.into(),
            })),
        })
    }
}

// ── ImProvider impl ───────────────────────────────────────────────────────────

#[async_trait]
impl ImProvider for DiscordProvider {
    /// Verify Ed25519 signature, then parse the Interaction payload.
    ///
    /// Returns `ImEvent::Challenge` for PING (type 1) so the gateway
    /// layer knows to reply with PONG without spawning an agent turn.
    async fn parse(&self, webhook: &Webhook) -> Result<ImEvent, ProviderError> {
        let sig_hex = webhook
            .header("x-signature-ed25519")
            .ok_or(ProviderError::BadSignature)?;
        let timestamp = webhook
            .header("x-signature-timestamp")
            .ok_or(ProviderError::BadSignature)?;

        signature::verify(&self.public_key, timestamp, &webhook.body, sig_hex)?;

        match interactions::parse_interaction(&webhook.body)? {
            // PING — map to the gateway's Challenge variant so the router
            // echoes the PONG without spawning an agent task.
            None => Ok(ImEvent::Challenge {
                challenge: "pong".into(),
            }),
            Some(event) => Ok(event),
        }
    }

    /// Send a reply back to Discord.
    ///
    /// The `conversation_id` is expected to be in the form produced by
    /// [`interactions::parse_interaction`]:
    ///
    /// - `discord:channel:<channel_id>` — sends to that channel.
    /// - `discord:dm:<user_id>` — falls back to `default_channel_id` (DM
    ///   channels require the `CREATE_DM` endpoint which is out of scope
    ///   for v1.2.8; a TODO note is emitted via tracing).
    async fn reply(&self, out: &OutgoingReply) -> Result<JsonValue, ProviderError> {
        match &self.reply_sink {
            ReplySink::Stub => {
                tracing::debug!(?out, "discord reply stub");
                Ok(json!({"status":"stubbed"}))
            }
            ReplySink::Recording(buf) => {
                buf.lock().push(out.clone());
                Ok(json!({"status":"recorded"}))
            }
            ReplySink::Api(sink) => {
                let channel_id = extract_channel_id(&out.conversation_id, &sink.default_channel_id);
                let resp = sink.client.send_message(channel_id, &out.text).await?;
                tracing::info!(channel = %channel_id, "discord reply sent");
                Ok(resp)
            }
        }
    }

    fn name(&self) -> &'static str {
        "discord"
    }
}

/// Extract the Discord channel id from the encoded `conversation_id`.
///
/// `discord:channel:<id>` → `<id>`
/// Anything else → `default_channel_id`
fn extract_channel_id<'a>(conversation_id: &'a str, default: &'a str) -> &'a str {
    conversation_id
        .strip_prefix("discord:channel:")
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer as _, SigningKey};
    use rand::rngs::OsRng;
    use serde_json::json;

    // ── Key helpers ──────────────────────────────────────────────────────────

    fn make_keypair() -> (SigningKey, VerifyingKey) {
        let sk = SigningKey::generate(&mut OsRng);
        let vk = sk.verifying_key();
        (sk, vk)
    }

    fn hex_vk(vk: &VerifyingKey) -> String {
        hex::encode(vk.as_bytes())
    }

    fn make_webhook(sk: &SigningKey, body: &[u8]) -> Webhook {
        let ts = "1716355200";
        let mut msg = Vec::new();
        msg.extend_from_slice(ts.as_bytes());
        msg.extend_from_slice(body);
        let sig = sk.sign(&msg);
        Webhook {
            headers: vec![
                ("x-signature-ed25519".into(), hex::encode(sig.to_bytes())),
                ("x-signature-timestamp".into(), ts.into()),
            ],
            body: body.to_vec(),
        }
    }

    // ── PING → Challenge ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn ping_returns_challenge_event() {
        let (sk, vk) = make_keypair();
        let provider = DiscordProvider::new(&hex_vk(&vk)).expect("valid key");
        let body = br#"{"id":"1","type":1}"#;
        let webhook = make_webhook(&sk, body);
        let event = provider.parse(&webhook).await.expect("parse");
        assert!(
            matches!(event, ImEvent::Challenge { .. }),
            "PING should produce Challenge"
        );
    }

    // ── Bad signature → BadSignature ─────────────────────────────────────────

    #[tokio::test]
    async fn bad_signature_rejected() {
        let (_, vk) = make_keypair();
        let (sk2, _) = make_keypair(); // different key
        let provider = DiscordProvider::new(&hex_vk(&vk)).expect("valid key");
        let body = br#"{"id":"2","type":1}"#;
        let webhook = make_webhook(&sk2, body);
        assert!(matches!(
            provider.parse(&webhook).await,
            Err(ProviderError::BadSignature)
        ));
    }

    // ── Missing headers → BadSignature ───────────────────────────────────────

    #[tokio::test]
    async fn missing_signature_header_rejected() {
        let (_, vk) = make_keypair();
        let provider = DiscordProvider::new(&hex_vk(&vk)).expect("valid key");
        let webhook = Webhook {
            headers: vec![("x-signature-timestamp".into(), "ts".into())],
            body: b"{}".to_vec(),
        };
        assert!(matches!(
            provider.parse(&webhook).await,
            Err(ProviderError::BadSignature)
        ));
    }

    // ── Slash command round-trip ─────────────────────────────────────────────

    #[tokio::test]
    async fn slash_command_parses_to_message_event() {
        let (sk, vk) = make_keypair();
        let provider = DiscordProvider::new(&hex_vk(&vk)).expect("valid key");
        let body = json!({
            "id": "cmd1",
            "type": 2,
            "guild_id": "g",
            "channel_id": "ch1",
            "member": { "user": { "id": "u1", "username": "alice" } },
            "data": { "id": "d1", "name": "ping", "options": [] }
        });
        let webhook = make_webhook(&sk, body.to_string().as_bytes());
        let event = provider.parse(&webhook).await.expect("parse");
        let ImEvent::Message(m) = event else {
            panic!("expected Message, got {event:?}");
        };
        assert_eq!(m.provider, "discord");
        assert_eq!(m.user_external_id, "u1");
        assert_eq!(m.conversation_id, "discord:channel:ch1");
        assert_eq!(m.text, "/ping");
    }

    // ── Reply recording sink ─────────────────────────────────────────────────

    #[tokio::test]
    async fn recording_sink_captures_reply() {
        let (_, vk) = make_keypair();
        let sink = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let provider =
            DiscordProvider::with_recording_sink(&hex_vk(&vk), sink.clone()).expect("valid key");
        provider
            .reply(&OutgoingReply {
                conversation_id: "discord:channel:ch42".into(),
                text: "hello user".into(),
            })
            .await
            .expect("reply ok");
        let buf = sink.lock();
        assert_eq!(buf.len(), 1);
        assert_eq!(buf[0].text, "hello user");
    }

    // ── API sink routes to correct channel ───────────────────────────────────

    #[tokio::test]
    async fn api_sink_sends_to_channel_from_conversation_id() {
        use crate::outbound::tests::FakeDiscordClient;

        let (_, vk) = make_keypair();
        let fake = FakeDiscordClient::new(json!({"id":"m1"}));
        let provider = DiscordProvider::with_api_sink(
            &hex_vk(&vk),
            fake.clone() as Arc<dyn DiscordClient>,
            "default_ch",
        )
        .expect("valid key");

        provider
            .reply(&OutgoingReply {
                conversation_id: "discord:channel:ch99".into(),
                text: "response text".into(),
            })
            .await
            .expect("reply ok");

        let calls = fake.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "ch99");
        assert_eq!(calls[0].1, "response text");
    }

    #[tokio::test]
    async fn api_sink_falls_back_to_default_channel_for_dm() {
        use crate::outbound::tests::FakeDiscordClient;

        let (_, vk) = make_keypair();
        let fake = FakeDiscordClient::new(json!({"id":"m2"}));
        let provider = DiscordProvider::with_api_sink(
            &hex_vk(&vk),
            fake.clone() as Arc<dyn DiscordClient>,
            "default_ch",
        )
        .expect("valid key");

        provider
            .reply(&OutgoingReply {
                conversation_id: "discord:dm:user_abc".into(),
                text: "dm reply".into(),
            })
            .await
            .expect("reply ok");

        let calls = fake.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].0, "default_ch",
            "DM should fall back to default channel"
        );
    }

    // ── extract_channel_id ───────────────────────────────────────────────────

    #[test]
    fn extract_channel_id_from_channel_conversation() {
        assert_eq!(
            extract_channel_id("discord:channel:abc123", "default"),
            "abc123"
        );
    }

    #[test]
    fn extract_channel_id_falls_back_for_dm() {
        assert_eq!(
            extract_channel_id("discord:dm:user1", "fallback_ch"),
            "fallback_ch"
        );
    }
}
