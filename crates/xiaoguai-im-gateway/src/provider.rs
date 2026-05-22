//! `ImProvider` — the abstraction every IM adapter implements.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use thiserror::Error;

/// Webhook payload after signature verification + parsing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ImEvent {
    /// Initial verification handshake (Feishu sends `{"challenge":"..."}`
    /// on URL configuration). Adapter must echo it.
    Challenge { challenge: String },
    /// A user-originated chat message.
    Message(IncomingMessage),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IncomingMessage {
    pub provider: String,
    /// Provider's user identifier (Feishu `open_id`, DingTalk `userid`, etc.)
    pub user_external_id: String,
    /// Provider's tenant identifier (Feishu `tenant_key`, DingTalk `corpid`).
    pub tenant_external_id: String,
    /// The conversation/channel/group id.
    pub conversation_id: String,
    pub text: String,
    /// Original event id; used for de-duplication.
    pub event_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutgoingReply {
    pub conversation_id: String,
    pub text: String,
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("signature verification failed")]
    BadSignature,
    #[error("malformed payload: {0}")]
    Malformed(String),
    #[error("provider transport error: {0}")]
    Transport(String),
}

/// Headers + body received from the webhook. Generic so adapters can
/// extract whichever headers they care about.
#[derive(Debug, Clone, Default)]
pub struct Webhook {
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Webhook {
    /// Look up a header by lowercase name. Returns the first match.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        let lower = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| k.to_ascii_lowercase() == lower)
            .map(|(_, v)| v.as_str())
    }

    /// UTF-8 view of the body. Adapters that need raw bytes use `body`.
    #[must_use]
    pub fn body_str(&self) -> &str {
        std::str::from_utf8(&self.body).unwrap_or("")
    }
}

#[async_trait]
pub trait ImProvider: Send + Sync {
    /// Verify signature + decode into a structured event. Implementations
    /// must reject on signature mismatch.
    async fn parse(&self, webhook: &Webhook) -> Result<ImEvent, ProviderError>;

    /// Push a reply back to the user. v0.7 implementations may stub this
    /// out — production wiring lands in v0.7.1.
    async fn reply(&self, out: &OutgoingReply) -> Result<JsonValue, ProviderError>;

    /// Provider name used in logs + `IncomingMessage::provider`.
    fn name(&self) -> &'static str;
}
