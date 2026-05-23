//! Feishu `OpenAPI` client + `tenant_access_token` cache.
//!
//! v0.7.1 talks to two endpoints:
//!
//!   - `POST /open-apis/auth/v3/tenant_access_token/internal`
//!     → exchanges `(app_id, app_secret)` for a `tenant_access_token`
//!     with a TTL in seconds.
//!   - `POST /open-apis/im/v1/messages?receive_id_type=chat_id`
//!     → sends an outbound message. Auth header is
//!     `Authorization: Bearer <token>`.
//!
//! The cache stores one token per process. Re-fetch fires when the
//! cached entry is missing or its TTL has expired (we refresh 60 s
//! before expiry so concurrent requests don't all race the wire). The
//! cache is wrapped in a `tokio::sync::Mutex` so the first caller after
//! expiry blocks the others until the new token arrives — single-flight
//! by construction.
//!
//! The HTTP transport sits behind the [`FeishuClient`] trait so tests
//! can swap in a fake without spinning up wiremock.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use tokio::sync::Mutex;

use xiaoguai_im_gateway::ProviderError;

/// Trait covering the minimum HTTP surface we need from Feishu.
///
/// Production wires this to [`HttpFeishuClient`]. Tests pass a fake.
#[async_trait]
pub trait FeishuClient: Send + Sync {
    /// Exchange app credentials for a `(token, expire_in_secs)` pair.
    /// `expire_in_secs` mirrors what Feishu returns ("expire").
    async fn fetch_tenant_access_token(
        &self,
        app_id: &str,
        app_secret: &str,
    ) -> Result<TokenResponse, ProviderError>;

    /// Send a chat message to `chat_id` as plain text.
    async fn send_text_message(
        &self,
        token: &str,
        chat_id: &str,
        text: &str,
    ) -> Result<JsonValue, ProviderError>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenResponse {
    pub token: String,
    pub expire_in_secs: i64,
}

/// Default `https://open.feishu.cn` host. Override for tests via
/// [`HttpFeishuClient::with_base_url`].
pub const DEFAULT_BASE_URL: &str = "https://open.feishu.cn";

/// Concrete reqwest-backed implementation of [`FeishuClient`].
#[derive(Clone)]
pub struct HttpFeishuClient {
    client: reqwest::Client,
    base_url: String,
}

impl HttpFeishuClient {
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
impl FeishuClient for HttpFeishuClient {
    async fn fetch_tenant_access_token(
        &self,
        app_id: &str,
        app_secret: &str,
    ) -> Result<TokenResponse, ProviderError> {
        #[derive(Deserialize)]
        struct Resp {
            // Feishu uses `code != 0` for errors. We surface the message.
            #[serde(default)]
            code: i64,
            #[serde(default)]
            msg: Option<String>,
            #[serde(default)]
            tenant_access_token: Option<String>,
            #[serde(default)]
            expire: Option<i64>,
        }
        let url = format!(
            "{}/open-apis/auth/v3/tenant_access_token/internal",
            self.base_url
        );
        let body = json!({"app_id": app_id, "app_secret": app_secret});
        let resp: Resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(format!("auth send: {e}")))?
            .json()
            .await
            .map_err(|e| ProviderError::Transport(format!("auth decode: {e}")))?;
        if resp.code != 0 {
            return Err(ProviderError::Transport(format!(
                "feishu auth error code={} msg={:?}",
                resp.code, resp.msg
            )));
        }
        let token = resp
            .tenant_access_token
            .ok_or_else(|| ProviderError::Transport("missing tenant_access_token".into()))?;
        let expire_in_secs = resp.expire.unwrap_or(7200);
        Ok(TokenResponse {
            token,
            expire_in_secs,
        })
    }

    async fn send_text_message(
        &self,
        token: &str,
        chat_id: &str,
        text: &str,
    ) -> Result<JsonValue, ProviderError> {
        let url = format!(
            "{}/open-apis/im/v1/messages?receive_id_type=chat_id",
            self.base_url
        );
        // Feishu's `content` field is a JSON string holding a payload —
        // double-encode the {"text": ...} so it arrives as a string.
        let content_str = serde_json::to_string(&json!({"text": text}))
            .map_err(|e| ProviderError::Transport(format!("encode content: {e}")))?;
        let body = json!({
            "receive_id": chat_id,
            "msg_type": "text",
            "content": content_str,
        });
        let raw = self
            .client
            .post(&url)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(format!("send msg: {e}")))?
            .json::<JsonValue>()
            .await
            .map_err(|e| ProviderError::Transport(format!("decode msg: {e}")))?;
        // Feishu envelopes responses as `{code, msg, data}`. `code != 0`
        // means the API rejected the request (auth, quota, bad chat_id).
        if let Some(code) = raw.get("code").and_then(JsonValue::as_i64) {
            if code != 0 {
                return Err(ProviderError::Transport(format!(
                    "feishu send error code={code} body={raw}"
                )));
            }
        }
        Ok(raw)
    }
}

/// In-memory `tenant_access_token` cache. Wraps a [`FeishuClient`] so
/// callers see one method (`get_token`) that does the right thing
/// (fetch + cache + refresh-before-expiry).
pub struct TokenCache {
    client: Arc<dyn FeishuClient>,
    app_id: String,
    app_secret: String,
    /// Refresh this many seconds before the cached token's `expires_at`.
    /// 60 s is the same headroom Feishu's own SDK uses.
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
    pub fn new(client: Arc<dyn FeishuClient>, app_id: String, app_secret: String) -> Self {
        Self {
            client,
            app_id,
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
        // Cache miss or near expiry — fetch fresh.
        let resp = self
            .client
            .fetch_tenant_access_token(&self.app_id, &self.app_secret)
            .await?;
        let expires_at = now + chrono::Duration::seconds(resp.expire_in_secs);
        *guard = Some(CachedToken {
            token: resp.token.clone(),
            expires_at,
        });
        Ok(resp.token)
    }

    /// Force a refresh on the next call. Useful for tests + for an
    /// admin "rotate now" path that we may expose later.
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
        send_calls: SyncMutex<Vec<(String, String, String)>>,
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
    impl FeishuClient for FakeClient {
        async fn fetch_tenant_access_token(
            &self,
            _app_id: &str,
            _app_secret: &str,
        ) -> Result<TokenResponse, ProviderError> {
            *self.token_calls.lock() += 1;
            Ok(self.token_to_return.lock().clone())
        }
        async fn send_text_message(
            &self,
            token: &str,
            chat_id: &str,
            text: &str,
        ) -> Result<JsonValue, ProviderError> {
            self.send_calls
                .lock()
                .push((token.to_string(), chat_id.to_string(), text.to_string()));
            Ok(json!({"code": 0, "msg": "ok"}))
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
        let cache = TokenCache::new(fake.clone(), "app".into(), "sec".into());
        let t1 = cache.get_token_at(ts(1_000_000)).await.unwrap();
        assert_eq!(t1, "tok_initial");
        assert_eq!(*fake.token_calls.lock(), 1);

        // Second call within TTL → no new fetch.
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
        let cache = TokenCache::new(fake.clone(), "app".into(), "sec".into())
            .with_refresh_headroom(chrono::Duration::seconds(10));
        let _ = cache.get_token_at(ts(1_000_000)).await.unwrap();
        // Inside the headroom (expires at 1_000_100, headroom 10s → refresh
        // when now > 1_000_090).
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
        let cache = TokenCache::new(fake.clone(), "app".into(), "sec".into());
        let _ = cache.get_token().await.unwrap();
        assert_eq!(*fake.token_calls.lock(), 1);
        cache.invalidate().await;
        let _ = cache.get_token().await.unwrap();
        assert_eq!(*fake.token_calls.lock(), 2);
    }
}
