//! Outbound Discord message sending via the REST API v10.
//!
//! Supports:
//! - `POST /api/v10/channels/{channel_id}/messages` — send a plain-text
//!   message to a specific channel, authenticated with a Bot token.
//!
//! The [`DiscordClient`] trait decouples the HTTP surface so tests can
//! drive the full provider with a fake without hitting the network.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use xiaoguai_im_gateway::ProviderError;

/// Default Discord REST API base URL.
pub const DEFAULT_BASE_URL: &str = "https://discord.com";

/// Minimum HTTP surface for Discord outbound messaging.
///
/// The production implementation is [`HttpDiscordClient`]. Tests inject a
/// fake via `Arc<dyn DiscordClient>`.
#[async_trait]
pub trait DiscordClient: Send + Sync {
    /// Send a plain-text message to `channel_id`. Returns the Discord
    /// `Message` object on success.
    async fn send_message(
        &self,
        channel_id: &str,
        content: &str,
    ) -> Result<JsonValue, ProviderError>;
}

/// Concrete `reqwest`-backed [`DiscordClient`].
#[derive(Clone)]
pub struct HttpDiscordClient {
    client: reqwest::Client,
    base_url: String,
    bot_token: String,
}

impl HttpDiscordClient {
    /// Create a client against [`DEFAULT_BASE_URL`].
    ///
    /// # Errors
    /// Returns [`ProviderError::Transport`] if the underlying `reqwest`
    /// client cannot be built (TLS init failure).
    pub fn new(bot_token: impl Into<String>) -> Result<Self, ProviderError> {
        Self::with_base_url(DEFAULT_BASE_URL.to_string(), bot_token)
    }

    /// Create a client with a custom base URL — used in tests against a
    /// local mock server.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_base_url(
        base_url: String,
        bot_token: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| ProviderError::Transport(format!("build reqwest client: {e}")))?;
        Ok(Self {
            client,
            base_url,
            bot_token: bot_token.into(),
        })
    }
}

#[async_trait]
impl DiscordClient for HttpDiscordClient {
    async fn send_message(
        &self,
        channel_id: &str,
        content: &str,
    ) -> Result<JsonValue, ProviderError> {
        let url = format!("{}/api/v10/channels/{channel_id}/messages", self.base_url);
        let body = json!({ "content": content });

        let raw = self
            .client
            .post(&url)
            .header("Authorization", format!("Bot {}", self.bot_token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(format!("discord send: {e}")))?;

        let status = raw.status();
        let value: JsonValue = raw
            .json()
            .await
            .map_err(|e| ProviderError::Transport(format!("discord decode response: {e}")))?;

        if !status.is_success() {
            return Err(ProviderError::Transport(format!(
                "discord API error status={status} body={value}"
            )));
        }
        Ok(value)
    }
}

/// Test double for [`DiscordClient`]. Exposed as `pub(crate)` so `lib.rs`
/// integration tests can reuse it without needing a separate test-helper crate.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::Arc;

    // ── Fake client ─────────────────────────────────────────────────────────

    #[derive(Default)]
    pub(crate) struct FakeDiscordClient {
        pub(crate) calls: Mutex<Vec<(String, String)>>,
        pub(crate) response: Mutex<JsonValue>,
    }

    impl FakeDiscordClient {
        pub(crate) fn new(response: JsonValue) -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                response: Mutex::new(response),
            })
        }
    }

    #[async_trait]
    impl DiscordClient for FakeDiscordClient {
        async fn send_message(
            &self,
            channel_id: &str,
            content: &str,
        ) -> Result<JsonValue, ProviderError> {
            self.calls
                .lock()
                .push((channel_id.to_string(), content.to_string()));
            Ok(self.response.lock().clone())
        }
    }

    // ── Tests ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn fake_client_records_calls() {
        let fake = FakeDiscordClient::new(json!({"id": "msg1"}));
        fake.send_message("ch123", "hello discord")
            .await
            .expect("ok");
        let calls = fake.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "ch123");
        assert_eq!(calls[0].1, "hello discord");
    }

    /// HTTP round-trip via mockito.
    #[tokio::test]
    async fn http_client_sends_bot_auth_header() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/v10/channels/ch999/messages")
            .match_header("authorization", "Bot test_bot_token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id":"discord_msg_id","content":"hello"}"#)
            .create_async()
            .await;

        let client = HttpDiscordClient::with_base_url(server.url(), "test_bot_token").unwrap();
        let resp = client
            .send_message("ch999", "hello")
            .await
            .expect("send ok");
        assert_eq!(resp["id"], "discord_msg_id");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn http_client_propagates_api_error() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/api/v10/channels/bad/messages")
            .with_status(403)
            .with_header("content-type", "application/json")
            .with_body(r#"{"code":50013,"message":"Missing Permissions"}"#)
            .create_async()
            .await;

        let client = HttpDiscordClient::with_base_url(server.url(), "tok").unwrap();
        let result = client.send_message("bad", "hi").await;
        assert!(matches!(result, Err(ProviderError::Transport(_))));
    }
}
