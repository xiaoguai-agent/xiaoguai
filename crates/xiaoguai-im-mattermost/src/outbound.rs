//! Mattermost outbound post creation.
//!
//! Mattermost's REST API v4 endpoint for posting a message is:
//!
//! ```text
//! POST {base_url}/api/v4/posts
//! Authorization: Bearer <bot_token>
//! Content-Type: application/json
//!
//! {"channel_id": "...", "message": "..."}
//! ```
//!
//! Reference: https://api.mattermost.com/#tag/posts/operation/CreatePost
//!
//! The [`MattermostPoster`] trait abstracts the HTTP layer so tests can
//! substitute a fake without network access.

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};

use xiaoguai_im_gateway::ProviderError;

/// Minimum HTTP surface needed for posting messages.
///
/// Implementations must be `Send + Sync` so they can be wrapped in `Arc`.
#[async_trait]
pub trait MattermostPoster: Send + Sync {
    /// Create a post in `channel_id` with the given `message` text.
    ///
    /// Returns the raw JSON body Mattermost responds with.
    async fn create_post(
        &self,
        channel_id: &str,
        message: &str,
    ) -> Result<JsonValue, ProviderError>;
}

/// Request body for `POST /api/v4/posts`.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreatePostRequest {
    pub channel_id: String,
    pub message: String,
}

/// Production [`MattermostPoster`] backed by `reqwest`.
pub struct HttpMattermostPoster {
    client: reqwest::Client,
    base_url: String,
    bot_token: String,
}

impl HttpMattermostPoster {
    /// Build a poster for the given `base_url` and `bot_token`.
    ///
    /// `base_url` should be the Mattermost server root, e.g.
    /// `https://mattermost.example.com`.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::Transport`] if the underlying `reqwest`
    /// client cannot be constructed (e.g. TLS init failure).
    pub fn new(
        base_url: impl Into<String>,
        bot_token: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| ProviderError::Transport(format!("build reqwest: {e}")))?;
        Ok(Self {
            client,
            base_url: base_url.into(),
            bot_token: bot_token.into(),
        })
    }
}

#[async_trait]
impl MattermostPoster for HttpMattermostPoster {
    async fn create_post(
        &self,
        channel_id: &str,
        message: &str,
    ) -> Result<JsonValue, ProviderError> {
        let url = format!("{}/api/v4/posts", self.base_url);
        let body = json!({
            "channel_id": channel_id,
            "message": message,
        });
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.bot_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(format!("create_post send: {e}")))?;

        let status = resp.status();
        let raw = resp
            .json::<JsonValue>()
            .await
            .map_err(|e| ProviderError::Transport(format!("create_post decode: {e}")))?;

        if !status.is_success() {
            return Err(ProviderError::Transport(format!(
                "mattermost create_post status={status} body={raw}"
            )));
        }
        Ok(raw)
    }
}

/// In-process fake poster — test-only, exposed at crate scope so sibling
/// modules' tests can construct one.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::Arc;

    /// Records every `create_post` call.
    #[derive(Default)]
    pub struct FakePoster {
        pub calls: Mutex<Vec<(String, String)>>,
        pub response: Mutex<JsonValue>,
    }

    impl FakePoster {
        pub fn new_arc() -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                response: Mutex::new(json!({"id": "post_abc", "message": "ok"})),
            })
        }
    }

    #[async_trait]
    impl MattermostPoster for FakePoster {
        async fn create_post(
            &self,
            channel_id: &str,
            message: &str,
        ) -> Result<JsonValue, ProviderError> {
            self.calls
                .lock()
                .push((channel_id.to_string(), message.to_string()));
            Ok(self.response.lock().clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakePoster;
    use super::*;

    #[tokio::test]
    async fn fake_poster_records_calls() {
        let poster = FakePoster::new_arc();
        poster.create_post("ch1", "hello there").await.unwrap();
        poster.create_post("ch2", "second msg").await.unwrap();

        let calls = poster.calls.lock();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], ("ch1".into(), "hello there".into()));
        assert_eq!(calls[1], ("ch2".into(), "second msg".into()));
    }

    #[tokio::test]
    async fn fake_poster_returns_configured_response() {
        let poster = FakePoster::new_arc();
        let resp = poster.create_post("ch1", "hi").await.unwrap();
        assert_eq!(resp["id"], "post_abc");
    }

    /// Exercises the HTTP poster against a mock server to verify request
    /// structure: correct URL, Authorization header, and JSON body.
    #[tokio::test]
    async fn http_poster_sends_correct_request() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/v4/posts")
            .match_header("authorization", "Bearer bot-token-123")
            .match_body(mockito::Matcher::JsonString(
                r#"{"channel_id":"ch_test","message":"hello mattermost"}"#.into(),
            ))
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id":"post_new","message":"hello mattermost"}"#)
            .create_async()
            .await;

        let poster =
            HttpMattermostPoster::new(server.url(), "bot-token-123").expect("build poster");
        let resp = poster
            .create_post("ch_test", "hello mattermost")
            .await
            .expect("create_post should succeed");

        assert_eq!(resp["id"], "post_new");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn http_poster_returns_transport_error_on_non_2xx() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/api/v4/posts")
            .with_status(403)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id":"","message":"Forbidden"}"#)
            .create_async()
            .await;

        let poster = HttpMattermostPoster::new(server.url(), "bad-token").expect("build");
        let err = poster
            .create_post("ch_test", "blocked")
            .await
            .expect_err("should fail on 403");

        assert!(matches!(err, ProviderError::Transport(_)));
    }
}
