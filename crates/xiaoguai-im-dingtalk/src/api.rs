//! DingTalk `OpenAPI` client + `access_token` cache.
//!
//! v1.1.3 talks to three endpoints on `https://api.dingtalk.com`:
//!
//!   - `POST /v1.0/oauth2/{corpId}/token` body `{"appKey","appSecret"}`
//!     → exchanges app credentials for an `accessToken` with a TTL in
//!     seconds.
//!   - `POST /v1.0/robot/oToMessages/batchSend` (single-chat reply).
//!     Auth header `x-acs-dingtalk-access-token: <token>`.
//!   - `POST /v1.0/robot/groupMessages/send` (group reply, requires
//!     the inbound `conversationId` cached from the webhook).
//!
//! Same single-flight cache shape as Feishu: one token per process,
//! refresh fires 60 s before expiry, calls during a refresh serialise
//! on the same mutex so we don't stampede the wire.
//!
//! The HTTP transport sits behind [`DingTalkClient`] so tests can swap
//! in a fake without spinning up wiremock.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use tokio::sync::Mutex;

use xiaoguai_im_gateway::ProviderError;

/// Trait covering the minimum HTTP surface we need from DingTalk.
///
/// Production wires this to [`HttpDingTalkClient`]. Tests pass a fake.
#[async_trait]
pub trait DingTalkClient: Send + Sync {
    /// Exchange app credentials for a `(token, expire_in_secs)` pair.
    async fn fetch_access_token(
        &self,
        app_key: &str,
        app_secret: &str,
    ) -> Result<TokenResponse, ProviderError>;

    /// Send a text message to a single user identified by `open_conversation_id`.
    /// Robot single-chat path.
    async fn send_single_text(
        &self,
        token: &str,
        robot_code: &str,
        user_ids: &[String],
        text: &str,
    ) -> Result<JsonValue, ProviderError>;

    /// Send a text message into a group `open_conversation_id`.
    async fn send_group_text(
        &self,
        token: &str,
        robot_code: &str,
        open_conversation_id: &str,
        text: &str,
    ) -> Result<JsonValue, ProviderError>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenResponse {
    pub token: String,
    pub expire_in_secs: i64,
}

/// Default `https://api.dingtalk.com` host. Override for tests via
/// [`HttpDingTalkClient::with_base_url`].
pub const DEFAULT_BASE_URL: &str = "https://api.dingtalk.com";

/// Concrete reqwest-backed implementation of [`DingTalkClient`].
#[derive(Clone)]
pub struct HttpDingTalkClient {
    client: reqwest::Client,
    base_url: String,
}

impl HttpDingTalkClient {
    /// New client against [`DEFAULT_BASE_URL`].
    ///
    /// # Errors
    /// Returns `ProviderError::Transport` if the underlying reqwest
    /// client cannot be built (e.g. TLS init failure).
    pub fn new() -> Result<Self, ProviderError> {
        Self::with_base_url(DEFAULT_BASE_URL.to_string())
    }

    /// New client pointed at a custom base URL — used by tests with a
    /// local mock server.
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
impl DingTalkClient for HttpDingTalkClient {
    async fn fetch_access_token(
        &self,
        app_key: &str,
        app_secret: &str,
    ) -> Result<TokenResponse, ProviderError> {
        #[derive(Deserialize)]
        struct Resp {
            #[serde(rename = "accessToken", default)]
            access_token: Option<String>,
            #[serde(rename = "expireIn", default)]
            expire_in: Option<i64>,
            #[serde(default)]
            code: Option<String>,
            #[serde(default)]
            message: Option<String>,
        }
        let url = format!("{}/v1.0/oauth2/accessToken", self.base_url);
        let body = json!({"appKey": app_key, "appSecret": app_secret});
        let raw = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(format!("auth send: {e}")))?;
        let status = raw.status();
        let parsed: Resp = raw
            .json()
            .await
            .map_err(|e| ProviderError::Transport(format!("auth decode: {e}")))?;
        if !status.is_success() || parsed.access_token.is_none() {
            return Err(ProviderError::Transport(format!(
                "dingtalk auth error status={} code={:?} msg={:?}",
                status, parsed.code, parsed.message
            )));
        }
        Ok(TokenResponse {
            token: parsed.access_token.unwrap_or_default(),
            expire_in_secs: parsed.expire_in.unwrap_or(7200),
        })
    }

    async fn send_single_text(
        &self,
        token: &str,
        robot_code: &str,
        user_ids: &[String],
        text: &str,
    ) -> Result<JsonValue, ProviderError> {
        let url = format!("{}/v1.0/robot/oToMessages/batchSend", self.base_url);
        // DingTalk wants `msgParam` as a JSON string, not an object —
        // identical pattern to Feishu's `content`.
        let msg_param = serde_json::to_string(&json!({"content": text}))
            .map_err(|e| ProviderError::Transport(format!("encode msgParam: {e}")))?;
        let body = json!({
            "robotCode": robot_code,
            "userIds": user_ids,
            "msgKey": "sampleText",
            "msgParam": msg_param,
        });
        self.post_with_token(&url, token, &body).await
    }

    async fn send_group_text(
        &self,
        token: &str,
        robot_code: &str,
        open_conversation_id: &str,
        text: &str,
    ) -> Result<JsonValue, ProviderError> {
        let url = format!("{}/v1.0/robot/groupMessages/send", self.base_url);
        let msg_param = serde_json::to_string(&json!({"content": text}))
            .map_err(|e| ProviderError::Transport(format!("encode msgParam: {e}")))?;
        let body = json!({
            "robotCode": robot_code,
            "openConversationId": open_conversation_id,
            "msgKey": "sampleText",
            "msgParam": msg_param,
        });
        self.post_with_token(&url, token, &body).await
    }
}

impl HttpDingTalkClient {
    async fn post_with_token(
        &self,
        url: &str,
        token: &str,
        body: &JsonValue,
    ) -> Result<JsonValue, ProviderError> {
        let raw = self
            .client
            .post(url)
            .header("x-acs-dingtalk-access-token", token)
            .json(body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(format!("send msg: {e}")))?;
        let status = raw.status();
        let value: JsonValue = raw
            .json()
            .await
            .map_err(|e| ProviderError::Transport(format!("decode msg: {e}")))?;
        if !status.is_success() {
            return Err(ProviderError::Transport(format!(
                "dingtalk send error status={status} body={value}"
            )));
        }
        Ok(value)
    }
}

/// In-memory `access_token` cache. Wraps a [`DingTalkClient`] so callers
/// see one method (`get_token`) that does the right thing (fetch +
/// cache + refresh-before-expiry). Mirrors `xiaoguai_im_feishu::TokenCache`.
pub struct TokenCache {
    client: Arc<dyn DingTalkClient>,
    app_key: String,
    app_secret: String,
    /// Refresh this many seconds before the cached token's `expires_at`.
    refresh_headroom: chrono::Duration,
    cached: Mutex<Option<CachedToken>>,
}

#[derive(Debug, Clone)]
struct CachedToken {
    token: String,
    expires_at: DateTime<Utc>,
}

impl TokenCache {
    #[must_use]
    pub fn new(client: Arc<dyn DingTalkClient>, app_key: String, app_secret: String) -> Self {
        Self {
            client,
            app_key,
            app_secret,
            refresh_headroom: chrono::Duration::seconds(60),
            cached: Mutex::new(None),
        }
    }

    /// Visible to tests so they don't need to wait an hour.
    #[must_use]
    pub fn with_refresh_headroom(mut self, h: chrono::Duration) -> Self {
        self.refresh_headroom = h;
        self
    }

    /// Return a valid token, fetching one if missing or near expiry.
    ///
    /// # Errors
    /// Bubbles up any [`ProviderError`] returned by the underlying client.
    pub async fn get_token(&self) -> Result<String, ProviderError> {
        self.get_token_at(Utc::now()).await
    }

    /// Test-friendly variant: same as [`get_token`] but with a caller-
    /// supplied clock reading so tests can simulate the passage of time.
    pub async fn get_token_at(&self, now: DateTime<Utc>) -> Result<String, ProviderError> {
        let mut guard = self.cached.lock().await;
        if let Some(c) = guard.as_ref() {
            if c.expires_at - self.refresh_headroom > now {
                return Ok(c.token.clone());
            }
        }
        let resp = self
            .client
            .fetch_access_token(&self.app_key, &self.app_secret)
            .await?;
        let expires_at = now + chrono::Duration::seconds(resp.expire_in_secs);
        *guard = Some(CachedToken {
            token: resp.token.clone(),
            expires_at,
        });
        Ok(resp.token)
    }

    /// Force a refresh on the next call.
    pub async fn invalidate(&self) {
        self.cached.lock().await.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex as SyncMutex;
    use std::sync::Arc;

    /// Test double that records calls and lets us decide what to return.
    #[derive(Default)]
    struct FakeClient {
        token_calls: SyncMutex<u32>,
        send_calls: SyncMutex<Vec<(String, String, String, String)>>,
        token_to_return: SyncMutex<TokenResponse>,
    }

    impl FakeClient {
        fn arc(initial: TokenResponse) -> Arc<Self> {
            Arc::new(Self {
                token_calls: SyncMutex::new(0),
                send_calls: SyncMutex::new(Vec::new()),
                token_to_return: SyncMutex::new(initial),
            })
        }
    }

    #[async_trait]
    impl DingTalkClient for FakeClient {
        async fn fetch_access_token(
            &self,
            _app_key: &str,
            _app_secret: &str,
        ) -> Result<TokenResponse, ProviderError> {
            *self.token_calls.lock() += 1;
            Ok(self.token_to_return.lock().clone())
        }
        async fn send_single_text(
            &self,
            token: &str,
            robot_code: &str,
            user_ids: &[String],
            text: &str,
        ) -> Result<JsonValue, ProviderError> {
            self.send_calls.lock().push((
                token.to_string(),
                format!("single:{robot_code}"),
                user_ids.join(","),
                text.to_string(),
            ));
            Ok(json!({"processQueryKey": "abc"}))
        }
        async fn send_group_text(
            &self,
            token: &str,
            robot_code: &str,
            open_conversation_id: &str,
            text: &str,
        ) -> Result<JsonValue, ProviderError> {
            self.send_calls.lock().push((
                token.to_string(),
                format!("group:{robot_code}"),
                open_conversation_id.to_string(),
                text.to_string(),
            ));
            Ok(json!({"processQueryKey": "def"}))
        }
    }

    fn ts(secs: i64) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(secs, 0).unwrap()
    }

    impl Default for TokenResponse {
        fn default() -> Self {
            Self {
                token: "tok_initial".into(),
                expire_in_secs: 7200,
            }
        }
    }

    #[tokio::test]
    async fn first_call_fetches_and_caches() {
        let fake = FakeClient::arc(TokenResponse::default());
        let cache = TokenCache::new(fake.clone(), "ak".into(), "sec".into());
        let t1 = cache.get_token_at(ts(1_000_000)).await.unwrap();
        assert_eq!(t1, "tok_initial");
        assert_eq!(*fake.token_calls.lock(), 1);
        let t2 = cache.get_token_at(ts(1_000_001)).await.unwrap();
        assert_eq!(t2, "tok_initial");
        assert_eq!(*fake.token_calls.lock(), 1, "cache must serve hot reads");
    }

    #[tokio::test]
    async fn refresh_kicks_in_near_expiry() {
        let fake = FakeClient::arc(TokenResponse {
            token: "tok_a".into(),
            expire_in_secs: 100,
        });
        let cache = TokenCache::new(fake.clone(), "ak".into(), "sec".into())
            .with_refresh_headroom(chrono::Duration::seconds(10));
        let _ = cache.get_token_at(ts(1_000_000)).await.unwrap();
        *fake.token_to_return.lock() = TokenResponse {
            token: "tok_b".into(),
            expire_in_secs: 100,
        };
        let t = cache.get_token_at(ts(1_000_095)).await.unwrap();
        assert_eq!(t, "tok_b");
        assert_eq!(*fake.token_calls.lock(), 2);
    }

    #[tokio::test]
    async fn invalidate_forces_refresh() {
        let fake = FakeClient::arc(TokenResponse::default());
        let cache = TokenCache::new(fake.clone(), "ak".into(), "sec".into());
        let _ = cache.get_token().await.unwrap();
        assert_eq!(*fake.token_calls.lock(), 1);
        cache.invalidate().await;
        let _ = cache.get_token().await.unwrap();
        assert_eq!(*fake.token_calls.lock(), 2);
    }

    /// mockito-driven round-trip for the HTTP impl. Proves we send the
    /// fields DingTalk expects + decode the success envelope.
    #[tokio::test]
    async fn http_client_round_trip_send_single() {
        let mut server = mockito::Server::new_async().await;
        let auth_mock = server
            .mock("POST", "/v1.0/oauth2/accessToken")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"accessToken":"tok_real","expireIn":7200}"#)
            .create_async()
            .await;
        let send_mock = server
            .mock("POST", "/v1.0/robot/oToMessages/batchSend")
            .match_header("x-acs-dingtalk-access-token", "tok_real")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"processQueryKey":"xyz"}"#)
            .create_async()
            .await;
        let client = HttpDingTalkClient::with_base_url(server.url()).unwrap();
        let tok = client
            .fetch_access_token("ak", "sec")
            .await
            .expect("auth ok");
        assert_eq!(tok.token, "tok_real");
        let resp = client
            .send_single_text(&tok.token, "robot_x", &["u1".into()], "hi")
            .await
            .expect("send ok");
        assert_eq!(resp["processQueryKey"], "xyz");
        auth_mock.assert_async().await;
        send_mock.assert_async().await;
    }

    #[tokio::test]
    async fn http_client_round_trip_send_group() {
        let mut server = mockito::Server::new_async().await;
        let send_mock = server
            .mock("POST", "/v1.0/robot/groupMessages/send")
            .match_header("x-acs-dingtalk-access-token", "tok_real")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"processQueryKey":"gid"}"#)
            .create_async()
            .await;
        let client = HttpDingTalkClient::with_base_url(server.url()).unwrap();
        let resp = client
            .send_group_text("tok_real", "robot_x", "cid_group", "hi group")
            .await
            .expect("send ok");
        assert_eq!(resp["processQueryKey"], "gid");
        send_mock.assert_async().await;
    }
}
