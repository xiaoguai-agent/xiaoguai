//! Slack outbound HTTP client — `chat.postMessage`.
//!
//! Wraps the Slack Web API endpoint
//! `https://slack.com/api/chat.postMessage` with:
//!
//! - a trait [`SlackClient`] for testability (swap in a fake without
//!   spinning up a real server),
//! - a concrete [`HttpSlackClient`] backed by reqwest,
//! - a thin helper that posts a plain-text message to any channel/DM.
//!
//! Authentication: Bot Token (`xoxb-…`) sent as
//! `Authorization: Bearer <bot_token>`.
//!
//! Slack always returns HTTP 200 even on application-level errors.
//! We check the `ok` field in the JSON envelope and surface failures as
//! [`ProviderError::Transport`].

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};

use xiaoguai_im_gateway::ProviderError;

/// Minimum surface of the Slack Web API this adapter uses.
///
/// Concrete impls: [`HttpSlackClient`] for production, fake structs in tests.
#[async_trait]
pub trait SlackClient: Send + Sync {
    /// Post a plain-text message to `channel` (channel id, user id, or
    /// `#channel-name`).
    async fn post_message(
        &self,
        bot_token: &str,
        channel: &str,
        text: &str,
    ) -> Result<JsonValue, ProviderError>;
}

/// Default Slack API base URL.
pub const DEFAULT_BASE_URL: &str = "https://slack.com";

/// Concrete reqwest-backed [`SlackClient`].
#[derive(Clone)]
pub struct HttpSlackClient {
    client: reqwest::Client,
    base_url: String,
}

impl HttpSlackClient {
    /// New client pointed at the real Slack API.
    ///
    /// # Errors
    /// Returns `ProviderError::Transport` if reqwest cannot be initialised
    /// (e.g. TLS init failure).
    pub fn new() -> Result<Self, ProviderError> {
        Self::with_base_url(DEFAULT_BASE_URL.to_string())
    }

    /// New client pointed at `base_url` — used in tests with a mock server.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_base_url(base_url: String) -> Result<Self, ProviderError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| ProviderError::Transport(format!("build reqwest: {e}")))?;
        Ok(Self { client, base_url })
    }
}

#[async_trait]
impl SlackClient for HttpSlackClient {
    async fn post_message(
        &self,
        bot_token: &str,
        channel: &str,
        text: &str,
    ) -> Result<JsonValue, ProviderError> {
        let url = format!("{}/api/chat.postMessage", self.base_url);
        let body = json!({ "channel": channel, "text": text });

        let raw = self
            .client
            .post(&url)
            .bearer_auth(bot_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(format!("chat.postMessage send: {e}")))?;

        // Slack always returns 200; parse the body and check `ok`.
        let value: JsonValue = raw
            .json()
            .await
            .map_err(|e| ProviderError::Transport(format!("chat.postMessage decode: {e}")))?;

        if value.get("ok").and_then(JsonValue::as_bool) == Some(true) {
            Ok(value)
        } else {
            let error_code = value
                .get("error")
                .and_then(JsonValue::as_str)
                .unwrap_or("unknown");
            Err(ProviderError::Transport(format!(
                "chat.postMessage error: {error_code}"
            )))
        }
    }
}

/// Deserialise shape for the `chat.postMessage` JSON response.
#[derive(Debug, Deserialize)]
pub struct PostMessageResponse {
    pub ok: bool,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub ts: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// mockito round-trip: sends the right Authorization header and body;
    /// parses a successful `ok: true` response.
    #[tokio::test]
    async fn post_message_happy_path() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat.postMessage")
            .match_header("authorization", "Bearer xoxb-test-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"channel":"C123","ts":"1716355200.000100","message":{"text":"hello"}}"#)
            .create_async()
            .await;

        let client = HttpSlackClient::with_base_url(server.url()).unwrap();
        let resp = client
            .post_message("xoxb-test-token", "C123", "hello")
            .await
            .expect("should succeed");
        assert_eq!(resp["ok"], true);
        assert_eq!(resp["channel"], "C123");
        mock.assert_async().await;
    }

    /// Slack returns `ok: false` with an `error` code — we surface it as
    /// `ProviderError::Transport`.
    #[tokio::test]
    async fn post_message_api_error_surfaces_as_transport() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/api/chat.postMessage")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":false,"error":"channel_not_found"}"#)
            .create_async()
            .await;

        let client = HttpSlackClient::with_base_url(server.url()).unwrap();
        match client
            .post_message("xoxb-bad-token", "C_INVALID", "hi")
            .await
        {
            Err(ProviderError::Transport(msg)) => {
                assert!(msg.contains("channel_not_found"), "got: {msg}");
            }
            other => panic!("expected Transport error, got {other:?}"),
        }
    }

    /// A fake [`SlackClient`] that records calls — mirrors the pattern used in
    /// dingtalk's Recorder and proves the trait is object-safe.
    #[tokio::test]
    async fn fake_client_records_calls() {
        use parking_lot::Mutex;
        use std::sync::Arc;

        #[derive(Default)]
        struct Recorder {
            calls: Mutex<Vec<(String, String, String)>>,
        }

        #[async_trait::async_trait]
        impl SlackClient for Recorder {
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

        let rec: Arc<Recorder> = Arc::new(Recorder::default());
        rec.post_message("tok", "C1", "hi").await.unwrap();
        rec.post_message("tok", "C2", "there").await.unwrap();
        let calls = rec.calls.lock();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], ("tok".into(), "C1".into(), "hi".into()));
        assert_eq!(calls[1].1, "C2");
    }
}
