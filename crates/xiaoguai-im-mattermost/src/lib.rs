//! Mattermost IM adapter.
//!
//! Implements [`ImProvider`] for Mattermost by:
//!
//! * **Inbound** (`incoming.rs`): verifies the shared `token` field in an
//!   outgoing webhook body (form-encoded) using constant-time comparison,
//!   then parses it into an [`ImEvent::Message`].
//! * **Slash commands** (`slash.rs`): same verification + parsing for slash
//!   command POST bodies.
//! * **Outbound** (`outbound.rs`): `POST /api/v4/posts` with Bearer auth
//!   to create reply messages.
//! * **WebSocket** (`websocket.rs`): stub module; full implementation
//!   deferred to v1.3.
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use xiaoguai_im_mattermost::{MattermostProvider, outbound::HttpMattermostPoster};
//!
//! let poster = Arc::new(
//!     HttpMattermostPoster::new("https://mm.example.com", "my-bot-token").unwrap(),
//! );
//! let provider = MattermostProvider::new(
//!     "https://mm.example.com",
//!     "my-bot-token",
//!     Some("my-webhook-token".into()),
//! ).with_poster(poster);
//! ```

#![forbid(unsafe_code)]

pub mod incoming;
pub mod outbound;
pub mod slash;
pub mod websocket;

use std::sync::Arc;

use async_trait::async_trait;
use outbound::MattermostPoster;
use serde_json::{json, Value as JsonValue};

use xiaoguai_im_gateway::{ImEvent, ImProvider, OutgoingReply, ProviderError, Webhook};

/// Mattermost IM adapter.
///
/// `webhook_token` is used to verify inbound outgoing webhooks and slash
/// commands. It is `Option` because some deployments only need outbound
/// posting (e.g. notification-only bots).
#[derive(Clone)]
pub struct MattermostProvider {
    base_url: String,
    bot_token: String,
    webhook_token: Option<String>,
    poster: Option<Arc<dyn MattermostPoster>>,
}

impl MattermostProvider {
    /// Create a provider. Call [`Self::with_poster`] to enable outbound
    /// replies; the default is a no-op stub.
    #[must_use]
    pub fn new(
        base_url: impl Into<String>,
        bot_token: impl Into<String>,
        webhook_token: Option<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            bot_token: bot_token.into(),
            webhook_token,
            poster: None,
        }
    }

    /// Attach a [`MattermostPoster`] for outbound replies.
    ///
    /// Without this, [`ImProvider::reply`] logs a warning and returns a
    /// `{"status":"stubbed"}` response.
    #[must_use]
    pub fn with_poster(mut self, poster: Arc<dyn MattermostPoster>) -> Self {
        self.poster = Some(poster);
        self
    }

    /// Access the configured `base_url`.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Access the bot token (redact in logs — never print directly).
    #[must_use]
    pub fn bot_token(&self) -> &str {
        &self.bot_token
    }
}

#[async_trait]
impl ImProvider for MattermostProvider {
    async fn parse(&self, webhook: &Webhook) -> Result<ImEvent, ProviderError> {
        let token = self
            .webhook_token
            .as_deref()
            .ok_or(ProviderError::BadSignature)?;
        incoming::parse(webhook, token)
    }

    async fn reply(&self, out: &OutgoingReply) -> Result<JsonValue, ProviderError> {
        if let Some(p) = &self.poster {
            let resp = p.create_post(&out.conversation_id, &out.text).await?;
            tracing::info!(
                channel_id = %out.conversation_id,
                "mattermost reply sent"
            );
            Ok(resp)
        } else {
            tracing::debug!(?out, "mattermost reply stub — no poster configured");
            Ok(json!({"status": "stubbed"}))
        }
    }

    fn name(&self) -> &'static str {
        "mattermost"
    }
}

/// Re-export of the audited constant-time comparison from
/// [`xiaoguai_im_common`]; used by [`incoming`] and [`slash`] to verify the
/// shared secret token without leaking timing information on partial matches.
pub(crate) use xiaoguai_im_common::constant_time_eq;

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------ //
    //  MattermostProvider — parse() path                                  //
    // ------------------------------------------------------------------ //

    fn webhook(body: &str) -> Webhook {
        Webhook {
            headers: vec![],
            body: body.as_bytes().to_vec(),
        }
    }

    #[tokio::test]
    async fn parse_happy_path() {
        let provider = MattermostProvider::new("http://mm", "bot", Some("tok".into()));
        let body = "token=tok&channel_id=ch&user_name=alice&text=hello&post_id=p1&team_id=t1";
        match provider.parse(&webhook(body)).await.expect("ok") {
            ImEvent::Message(m) => {
                assert_eq!(m.provider, "mattermost");
                assert_eq!(m.user_external_id, "alice");
                assert_eq!(m.conversation_id, "ch");
                assert_eq!(m.text, "hello");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn parse_wrong_token_is_bad_signature() {
        let provider = MattermostProvider::new("http://mm", "bot", Some("tok".into()));
        let body = "token=wrong&channel_id=ch&user_name=alice&text=hi";
        assert!(matches!(
            provider.parse(&webhook(body)).await,
            Err(ProviderError::BadSignature)
        ));
    }

    #[tokio::test]
    async fn parse_without_webhook_token_configured_is_bad_signature() {
        // No webhook_token → we cannot verify → always reject.
        let provider = MattermostProvider::new("http://mm", "bot", None);
        let body = "token=tok&channel_id=ch&user_name=alice&text=hi";
        assert!(matches!(
            provider.parse(&webhook(body)).await,
            Err(ProviderError::BadSignature)
        ));
    }

    // ------------------------------------------------------------------ //
    //  MattermostProvider — reply() path                                  //
    // ------------------------------------------------------------------ //

    #[tokio::test]
    async fn reply_stub_returns_stubbed_status() {
        let provider = MattermostProvider::new("http://mm", "bot", None);
        let resp = provider
            .reply(&OutgoingReply {
                conversation_id: "ch".into(),
                text: "hello".into(),
            })
            .await
            .expect("stub reply");
        assert_eq!(resp["status"], "stubbed");
    }

    #[tokio::test]
    async fn reply_with_poster_delegates_correctly() {
        use outbound::fake::FakePoster;

        let fake = FakePoster::new_arc();
        let provider = MattermostProvider::new("http://mm", "bot", None)
            .with_poster(fake.clone() as Arc<dyn MattermostPoster>);

        provider
            .reply(&OutgoingReply {
                conversation_id: "ch_x".into(),
                text: "delegated".into(),
            })
            .await
            .expect("reply");

        let calls = fake.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], ("ch_x".into(), "delegated".into()));
    }

    #[test]
    fn name_returns_mattermost() {
        let provider = MattermostProvider::new("http://mm", "bot", None);
        assert_eq!(provider.name(), "mattermost");
    }
}
