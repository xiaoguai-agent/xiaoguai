//! WeCom (企业微信) `OpenAPI` client + `access_token` cache.
//!
//! v1.1.3 talks to two endpoints on `https://qyapi.weixin.qq.com`:
//!
//!   - `GET /cgi-bin/gettoken?corpid={corp_id}&corpsecret={corp_secret}`
//!     → exchanges corp credentials for an `access_token` with TTL in
//!     seconds.
//!   - `POST /cgi-bin/message/send?access_token={token}` with a JSON
//!     body identifying the target user / party / tag and the text
//!     content.
//!
//! Same single-flight cache shape as Feishu / DingTalk.
//!
//! The HTTP transport sits behind [`WeComClient`] so tests can swap in
//! a fake without spinning up wiremock.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use tokio::sync::Mutex;

use xiaoguai_im_gateway::ProviderError;

/// Trait covering the minimum HTTP surface we need from WeCom.
///
/// Production wires this to [`HttpWeComClient`]. Tests pass a fake.
#[async_trait]
pub trait WeComClient: Send + Sync {
    /// Exchange corp credentials for a `(token, expire_in_secs)` pair.
    async fn fetch_access_token(
        &self,
        corp_id: &str,
        corp_secret: &str,
    ) -> Result<TokenResponse, ProviderError>;

    /// Send a text message to a single user (touser) within the given
    /// `agent_id`.
    async fn send_text(
        &self,
        token: &str,
        agent_id: i64,
        touser: &str,
        text: &str,
    ) -> Result<JsonValue, ProviderError>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenResponse {
    pub token: String,
    pub expire_in_secs: i64,
}

/// Default `https://qyapi.weixin.qq.com` host. Override for tests via
/// [`HttpWeComClient::with_base_url`].
pub const DEFAULT_BASE_URL: &str = "https://qyapi.weixin.qq.com";

/// Concrete reqwest-backed implementation of [`WeComClient`].
#[derive(Clone)]
pub struct HttpWeComClient {
    client: reqwest::Client,
    base_url: String,
}

impl HttpWeComClient {
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
impl WeComClient for HttpWeComClient {
    async fn fetch_access_token(
        &self,
        corp_id: &str,
        corp_secret: &str,
    ) -> Result<TokenResponse, ProviderError> {
        #[derive(Deserialize)]
        struct Resp {
            #[serde(default)]
            errcode: i64,
            #[serde(default)]
            errmsg: Option<String>,
            #[serde(default)]
            access_token: Option<String>,
            #[serde(default)]
            expires_in: Option<i64>,
        }
        // SEC-14: WeCom's gettoken is a GET-only endpoint, so `corpsecret` must
        // ride in the query string. reqwest's error Display appends the failing
        // URL (query included) and does NOT redact query params, which would
        // leak the corp secret into `ProviderError::Transport` and any log that
        // records it. Strip the URL from transport errors with `without_url()`
        // before stringifying (same defence as the Gemini backend, SEC-04).
        let url = format!(
            "{}/cgi-bin/gettoken?corpid={}&corpsecret={}",
            self.base_url, corp_id, corp_secret
        );
        let resp: Resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(format!("auth send: {}", e.without_url())))?
            .json()
            .await
            .map_err(|e| ProviderError::Transport(format!("auth decode: {}", e.without_url())))?;
        if resp.errcode != 0 {
            return Err(ProviderError::Transport(format!(
                "wecom auth error errcode={} errmsg={:?}",
                resp.errcode, resp.errmsg
            )));
        }
        let token = resp
            .access_token
            .ok_or_else(|| ProviderError::Transport("missing access_token".into()))?;
        let expire_in_secs = resp.expires_in.unwrap_or(7200);
        Ok(TokenResponse {
            token,
            expire_in_secs,
        })
    }

    async fn send_text(
        &self,
        token: &str,
        agent_id: i64,
        touser: &str,
        text: &str,
    ) -> Result<JsonValue, ProviderError> {
        let url = format!(
            "{}/cgi-bin/message/send?access_token={}",
            self.base_url, token
        );
        let body = json!({
            "touser": touser,
            "msgtype": "text",
            "agentid": agent_id,
            "text": {"content": text},
            "safe": 0,
        });
        let raw: JsonValue = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(format!("send msg: {e}")))?
            .json()
            .await
            .map_err(|e| ProviderError::Transport(format!("decode msg: {e}")))?;
        if let Some(code) = raw.get("errcode").and_then(JsonValue::as_i64) {
            if code != 0 {
                return Err(ProviderError::Transport(format!(
                    "wecom send error errcode={code} body={raw}"
                )));
            }
        }
        Ok(raw)
    }
}

/// In-memory `access_token` cache. Wraps a [`WeComClient`] so callers
/// see one method (`get_token`) that does the right thing (fetch +
/// cache + refresh-before-expiry). Mirrors
/// `xiaoguai_im_feishu::TokenCache`.
pub struct TokenCache {
    client: Arc<dyn WeComClient>,
    corp_id: String,
    corp_secret: String,
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
    pub fn new(client: Arc<dyn WeComClient>, corp_id: String, corp_secret: String) -> Self {
        Self {
            client,
            corp_id,
            corp_secret,
            refresh_headroom: chrono::Duration::seconds(60),
            cached: Mutex::new(None),
        }
    }

    #[must_use]
    pub fn with_refresh_headroom(mut self, h: chrono::Duration) -> Self {
        self.refresh_headroom = h;
        self
    }

    /// # Errors
    /// Returns `ProviderError` if the token fetch from the WeCom API fails.
    pub async fn get_token(&self) -> Result<String, ProviderError> {
        self.get_token_at(Utc::now()).await
    }

    /// # Errors
    /// Returns `ProviderError` if the token fetch from the WeCom API fails.
    pub async fn get_token_at(&self, now: DateTime<Utc>) -> Result<String, ProviderError> {
        let mut guard = self.cached.lock().await;
        if let Some(c) = guard.as_ref() {
            if c.expires_at - self.refresh_headroom > now {
                return Ok(c.token.clone());
            }
        }
        let resp = self
            .client
            .fetch_access_token(&self.corp_id, &self.corp_secret)
            .await?;
        let expires_at = now + chrono::Duration::seconds(resp.expire_in_secs);
        *guard = Some(CachedToken {
            token: resp.token.clone(),
            expires_at,
        });
        Ok(resp.token)
    }

    pub async fn invalidate(&self) {
        self.cached.lock().await.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex as SyncMutex;
    use std::sync::Arc;

    #[derive(Default)]
    struct FakeClient {
        token_calls: SyncMutex<u32>,
        send_calls: SyncMutex<Vec<(String, i64, String, String)>>,
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
    impl WeComClient for FakeClient {
        async fn fetch_access_token(
            &self,
            _corp_id: &str,
            _corp_secret: &str,
        ) -> Result<TokenResponse, ProviderError> {
            *self.token_calls.lock() += 1;
            Ok(self.token_to_return.lock().clone())
        }
        async fn send_text(
            &self,
            token: &str,
            agent_id: i64,
            touser: &str,
            text: &str,
        ) -> Result<JsonValue, ProviderError> {
            self.send_calls.lock().push((
                token.to_string(),
                agent_id,
                touser.to_string(),
                text.to_string(),
            ));
            Ok(json!({"errcode": 0, "errmsg": "ok"}))
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
        let cache = TokenCache::new(fake.clone(), "corp".into(), "sec".into());
        let t1 = cache.get_token_at(ts(1_000_000)).await.unwrap();
        assert_eq!(t1, "tok_initial");
        assert_eq!(*fake.token_calls.lock(), 1);
        let _ = cache.get_token_at(ts(1_000_001)).await.unwrap();
        assert_eq!(*fake.token_calls.lock(), 1, "cache must serve hot reads");
    }

    #[tokio::test]
    async fn refresh_kicks_in_near_expiry() {
        let fake = FakeClient::arc(TokenResponse {
            token: "tok_a".into(),
            expire_in_secs: 100,
        });
        let cache = TokenCache::new(fake.clone(), "corp".into(), "sec".into())
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
        let cache = TokenCache::new(fake.clone(), "corp".into(), "sec".into());
        let _ = cache.get_token().await.unwrap();
        cache.invalidate().await;
        let _ = cache.get_token().await.unwrap();
        assert_eq!(*fake.token_calls.lock(), 2);
    }

    /// mockito-driven round-trip for the HTTP impl. Proves we send the
    /// right query string + headers and decode the success envelope.
    #[tokio::test]
    async fn http_client_round_trip() {
        let mut server = mockito::Server::new_async().await;
        let auth_mock = server
            .mock("GET", "/cgi-bin/gettoken")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("corpid".into(), "corp_x".into()),
                mockito::Matcher::UrlEncoded("corpsecret".into(), "sec_x".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"errcode":0,"errmsg":"ok","access_token":"tok_real","expires_in":7200}"#)
            .create_async()
            .await;
        let send_mock = server
            .mock("POST", "/cgi-bin/message/send")
            .match_query(mockito::Matcher::UrlEncoded(
                "access_token".into(),
                "tok_real".into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"errcode":0,"errmsg":"ok","msgid":"x"}"#)
            .create_async()
            .await;
        let client = HttpWeComClient::with_base_url(server.url()).unwrap();
        let tok = client
            .fetch_access_token("corp_x", "sec_x")
            .await
            .expect("auth ok");
        assert_eq!(tok.token, "tok_real");
        let resp = client
            .send_text(&tok.token, 1, "userA", "hi")
            .await
            .expect("send ok");
        assert_eq!(resp["errcode"], 0);
        auth_mock.assert_async().await;
        send_mock.assert_async().await;
    }
}
