//! DingTalk IM adapter.
//!
//! v1.1.3 ships the inbound + outbound path mirroring `xiaoguai-im-feishu`:
//! webhook signature verification, payload parsing into
//! `xiaoguai_im_gateway::ImEvent`, and an `OpenAPI` reply sink with
//! `access_token` caching.
//!
//! Signature shape DingTalk uses (Stream / `OpenAPI` webhook):
//! ```text
//! string_to_sign = timestamp + "\n" + app_secret
//! sig = base64(hmac_sha256(app_secret, string_to_sign))
//! ```
//! Delivered as `sign` query parameter (URL-encoded). Both `timestamp`
//! and `sign` arrive in the URL query string; the adapter accepts them
//! either via the `Webhook::headers` map (where the router copies query
//! pairs as `x-query-<name>` headers for transport convenience) or via
//! a future direct query-parameter capture (TBD when the gateway grows
//! a query-aware mount helper).
//!
//! For v1.1.3 the gateway forwards the raw query string as the
//! `x-dingtalk-query` header so the adapter can parse it itself.
//! Adapters that disable the query-string fallback can also send
//! `X-Dingtalk-Timestamp` + `X-Dingtalk-Sign` headers directly (used
//! by the integration tests).
//!
//! Requests are rejected as `ProviderError::BadSignature` if anything
//! mismatches, or if `timestamp` is outside the replay window
//! ([`TIMESTAMP_TOLERANCE_SECS`]). The DingTalk signature does **not**
//! cover the request body, so timestamp freshness is the only replay
//! mitigation available — a captured request stays valid for the whole
//! window, but no longer (SEC-05).
//!
//! Payload shape (single + group):
//! ```json
//! {
//!   "msgtype": "text",
//!   "text": {"content": "hello"},
//!   "conversationId": "cid_xxx",
//!   "conversationType": "1" | "2",      // 1 = single, 2 = group
//!   "senderId": "user_open_id",
//!   "senderCorpId": "ding_corp_xxx",
//!   "msgId": "msg_xxx",
//!   "robotCode": "ding_robot_xxx",
//!   "chatbotUserId": "ding_bot_user_xxx"
//! }
//! ```

#![forbid(unsafe_code)]

pub mod api;
pub mod stream;

pub use stream::{run_stream, InboundMessage, OutboundReply, StreamClient};

use std::sync::Arc;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as B64_STANDARD;
use base64::Engine as _;
use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use sha2::Sha256;
use xiaoguai_im_gateway::{
    ImEvent, ImProvider, IncomingMessage, OutgoingReply, ProviderError, Webhook,
};

pub use api::{DingTalkClient, HttpDingTalkClient, TokenCache, TokenResponse, DEFAULT_BASE_URL};

type HmacSha256 = Hmac<Sha256>;

/// SEC-05/SEC-12: maximum clock skew (seconds) allowed between DingTalk's
/// `timestamp` parameter and the current wall clock. Because the DingTalk
/// signature does not cover the body, this freshness window is the only
/// replay defence — mirrors Slack's 5-minute guidance.
pub const TIMESTAMP_TOLERANCE_SECS: i64 = 300;

/// Current Unix time in seconds. Falls back to 0 when the system clock
/// reports a pre-epoch time, which pushes every inbound timestamp outside
/// the replay window (fail-closed).
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[derive(Clone)]
pub struct DingTalkProvider {
    app_secret: String,
    reply_sink: ReplySink,
}

#[derive(Clone, Default)]
pub enum ReplySink {
    /// Discard the reply. Used in tests + dev.
    #[default]
    Stub,
    /// In-memory recorder so tests can assert on what would have been sent.
    Recording(std::sync::Arc<parking_lot::Mutex<Vec<OutgoingReply>>>),
    /// Send via the real DingTalk `OpenAPI`.
    Api(Arc<ApiSink>),
}

/// Holds the HTTP client + token cache + robot identity for the
/// real-`OpenAPI` reply path.
pub struct ApiSink {
    pub client: Arc<dyn DingTalkClient>,
    pub token_cache: TokenCache,
    /// DingTalk's `robotCode` — required by both reply endpoints.
    pub robot_code: String,
}

impl ApiSink {
    #[must_use]
    pub fn new(
        client: Arc<dyn DingTalkClient>,
        app_key: String,
        app_secret: String,
        robot_code: String,
    ) -> Self {
        let token_cache = TokenCache::new(Arc::clone(&client), app_key, app_secret);
        Self {
            client,
            token_cache,
            robot_code,
        }
    }
}

impl DingTalkProvider {
    #[must_use]
    pub fn new(app_secret: impl Into<String>) -> Self {
        Self {
            app_secret: app_secret.into(),
            reply_sink: ReplySink::Stub,
        }
    }

    #[must_use]
    pub fn with_recording_sink(
        app_secret: impl Into<String>,
        sink: std::sync::Arc<parking_lot::Mutex<Vec<OutgoingReply>>>,
    ) -> Self {
        Self {
            app_secret: app_secret.into(),
            reply_sink: ReplySink::Recording(sink),
        }
    }

    /// Build a provider that sends replies through the real DingTalk
    /// `OpenAPI`. The `client` parameter is generic over [`DingTalkClient`]
    /// so tests can drive the full provider without hitting the network.
    #[must_use]
    pub fn with_api_sink(
        app_secret: impl Into<String>,
        client: Arc<dyn DingTalkClient>,
        app_key: impl Into<String>,
        api_app_secret: impl Into<String>,
        robot_code: impl Into<String>,
    ) -> Self {
        Self {
            app_secret: app_secret.into(),
            reply_sink: ReplySink::Api(Arc::new(ApiSink::new(
                client,
                app_key.into(),
                api_app_secret.into(),
                robot_code.into(),
            ))),
        }
    }
}

/// Verify a DingTalk webhook signature.
///
/// Reads `timestamp` + `sign` from one of:
///   - explicit headers `X-Dingtalk-Timestamp` + `X-Dingtalk-Sign`
///     (preferred — what the gateway should be wiring),
///   - or the `x-dingtalk-query` header (URL-encoded query string)
///     to support transports that don't expose individual query keys.
///
/// `now_unix` is the current Unix timestamp in **seconds**; pass
/// [`now_unix()`] in production and a fixed value in tests.
fn verify(webhook: &Webhook, app_secret: &str, now_unix: i64) -> Result<(), ProviderError> {
    let (timestamp, signature) = read_sig_pair(webhook).ok_or(ProviderError::BadSignature)?;

    // SEC-05/SEC-12 replay protection: DingTalk's `timestamp` is
    // MILLISECONDS since the Unix epoch — convert to seconds before
    // windowing. Anything unparsable or outside ±TIMESTAMP_TOLERANCE_SECS
    // is rejected (fail-closed).
    let ts_millis: i64 = timestamp.parse().map_err(|_| ProviderError::BadSignature)?;
    let ts_secs = ts_millis / 1000;
    if (ts_secs - now_unix).abs() > TIMESTAMP_TOLERANCE_SECS {
        return Err(ProviderError::BadSignature);
    }

    let string_to_sign = format!("{timestamp}\n{app_secret}");
    let mut mac = HmacSha256::new_from_slice(app_secret.as_bytes())
        .map_err(|_| ProviderError::BadSignature)?;
    mac.update(string_to_sign.as_bytes());
    let computed = B64_STANDARD.encode(mac.finalize().into_bytes());

    if constant_time_eq(computed.as_bytes(), signature.as_bytes()) {
        Ok(())
    } else {
        Err(ProviderError::BadSignature)
    }
}

/// Extract `(timestamp, sign)` from either dedicated headers or the
/// URL-encoded query-string fallback. Returns `None` if neither has
/// both fields.
fn read_sig_pair(webhook: &Webhook) -> Option<(String, String)> {
    if let (Some(ts), Some(sig)) = (
        webhook.header("X-Dingtalk-Timestamp"),
        webhook.header("X-Dingtalk-Sign"),
    ) {
        return Some((ts.to_string(), sig.to_string()));
    }
    let query = webhook.header("x-dingtalk-query")?;
    let mut ts: Option<String> = None;
    let mut sig: Option<String> = None;
    for pair in query.split('&') {
        let (key, val) = pair.split_once('=')?;
        let decoded = urlencoding::decode(val).ok()?.into_owned();
        match key {
            "timestamp" => ts = Some(decoded),
            "sign" => sig = Some(decoded),
            _ => {}
        }
    }
    Some((ts?, sig?))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[derive(Deserialize)]
struct DingTalkPayload {
    #[serde(default)]
    msgtype: Option<String>,
    #[serde(default)]
    text: Option<TextBody>,
    #[serde(rename = "conversationId", default)]
    conversation_id: Option<String>,
    #[serde(rename = "conversationType", default)]
    conversation_type: Option<String>,
    #[serde(rename = "senderId", default)]
    sender_id: Option<String>,
    #[serde(rename = "senderStaffId", default)]
    sender_staff_id: Option<String>,
    #[serde(rename = "senderCorpId", default)]
    sender_corp_id: Option<String>,
    #[serde(rename = "msgId", default)]
    msg_id: Option<String>,
}

#[derive(Deserialize)]
struct TextBody {
    #[serde(default)]
    content: String,
}

fn parse_event(body: &[u8]) -> Result<ImEvent, ProviderError> {
    let p: DingTalkPayload = serde_json::from_slice(body)
        .map_err(|e| ProviderError::Malformed(format!("decode payload: {e}")))?;

    // DingTalk only emits text messages from the chatbot inbound webhook;
    // other types (image, audio) are out of scope for v1.1.3.
    match p.msgtype.as_deref() {
        Some("text") => {}
        Some(other) => {
            return Err(ProviderError::Malformed(format!(
                "unsupported msgtype: {other}"
            )))
        }
        None => return Err(ProviderError::Malformed("missing msgtype".into())),
    }

    let conversation_id = p
        .conversation_id
        .ok_or_else(|| ProviderError::Malformed("missing conversationId".into()))?;
    // Prefer the staff id (cross-tenant stable) over the per-conversation
    // senderId.
    let user_external_id = p
        .sender_staff_id
        .or(p.sender_id)
        .ok_or_else(|| ProviderError::Malformed("missing sender id".into()))?;
    let tenant_external_id = p
        .sender_corp_id
        .ok_or_else(|| ProviderError::Malformed("missing senderCorpId".into()))?;
    let event_id = p
        .msg_id
        .ok_or_else(|| ProviderError::Malformed("missing msgId".into()))?;
    let text = p.text.map(|t| t.content).unwrap_or_default();
    // conversation_type "1" = single, "2" = group. We keep both flows
    // distinct by sending replies through different endpoints based on
    // a `dingtalk:single:` / `dingtalk:group:` prefix on the
    // conversation id so the reply path can pick.
    let prefix = match p.conversation_type.as_deref() {
        Some("2") => "dingtalk:group:",
        _ => "dingtalk:single:",
    };
    let conversation_id = format!("{prefix}{conversation_id}|{user_external_id}");

    Ok(ImEvent::Message(IncomingMessage {
        provider: "dingtalk".into(),
        user_external_id,
        tenant_external_id,
        conversation_id,
        text,
        event_id,
    }))
}

/// Decode the conversation-id encoding produced by [`parse_event`] back
/// into `(kind, raw_conversation_id, user_id)`. Pub so tests can assert
/// on it directly and the reply path knows whether to call the single
/// or group endpoint.
#[must_use]
pub fn split_conversation_id(id: &str) -> Option<(ConversationKind, &str, &str)> {
    let (kind, rest) = if let Some(rest) = id.strip_prefix("dingtalk:single:") {
        (ConversationKind::Single, rest)
    } else if let Some(rest) = id.strip_prefix("dingtalk:group:") {
        (ConversationKind::Group, rest)
    } else {
        return None;
    };
    let (conv, user) = rest.split_once('|')?;
    Some((kind, conv, user))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationKind {
    Single,
    Group,
}

#[async_trait]
impl ImProvider for DingTalkProvider {
    async fn parse(&self, webhook: &Webhook) -> Result<ImEvent, ProviderError> {
        // SEC-05: the only call-site that reads the system clock — the
        // lower-level `verify` stays clock-injectable for tests.
        verify(webhook, &self.app_secret, now_unix())?;
        parse_event(&webhook.body)
    }

    async fn reply(&self, out: &OutgoingReply) -> Result<JsonValue, ProviderError> {
        match &self.reply_sink {
            ReplySink::Stub => {
                tracing::debug!(?out, "dingtalk reply stub");
                Ok(json!({"status":"stubbed"}))
            }
            ReplySink::Recording(buf) => {
                buf.lock().push(out.clone());
                Ok(json!({"status":"recorded"}))
            }
            ReplySink::Api(sink) => {
                let token = sink.token_cache.get_token().await?;
                let (kind, conv, user) =
                    split_conversation_id(&out.conversation_id).ok_or_else(|| {
                        ProviderError::Transport(format!(
                            "unrecognised dingtalk conversation_id format: {}",
                            out.conversation_id
                        ))
                    })?;
                let resp = match kind {
                    ConversationKind::Single => {
                        sink.client
                            .send_single_text(
                                &token,
                                &sink.robot_code,
                                &[user.to_string()],
                                &out.text,
                            )
                            .await?
                    }
                    ConversationKind::Group => {
                        sink.client
                            .send_group_text(&token, &sink.robot_code, conv, &out.text)
                            .await?
                    }
                };
                tracing::info!(conv = %conv, kind = ?kind, "dingtalk reply sent");
                Ok(resp)
            }
        }
    }

    fn name(&self) -> &'static str {
        "dingtalk"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig_for(ts: &str, secret: &str) -> String {
        let string_to_sign = format!("{ts}\n{secret}");
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(string_to_sign.as_bytes());
        B64_STANDARD.encode(mac.finalize().into_bytes())
    }

    /// Build a webhook signed for an explicit millisecond timestamp so
    /// freshness tests can place it precisely relative to `now`.
    fn signed_webhook_at(body: &str, secret: &str, ts_millis: i64) -> Webhook {
        let ts = ts_millis.to_string();
        let sig = sig_for(&ts, secret);
        Webhook {
            headers: vec![
                ("X-Dingtalk-Timestamp".into(), ts),
                ("X-Dingtalk-Sign".into(), sig),
            ],
            body: body.as_bytes().to_vec(),
        }
    }

    /// Webhook signed "now" — survives the SEC-05 freshness window when
    /// the full `provider.parse` path reads the real clock.
    fn signed_webhook(body: &str, secret: &str) -> Webhook {
        signed_webhook_at(body, secret, now_unix() * 1000)
    }

    #[tokio::test]
    async fn verify_passes_with_matching_signature() {
        let webhook = signed_webhook(r#"{"msgtype":"text"}"#, "secret");
        verify(&webhook, "secret", now_unix()).expect("verify");
    }

    #[tokio::test]
    async fn verify_fails_with_wrong_secret() {
        let webhook = signed_webhook(r#"{"msgtype":"text"}"#, "secret");
        assert!(matches!(
            verify(&webhook, "different-secret", now_unix()),
            Err(ProviderError::BadSignature)
        ));
    }

    #[tokio::test]
    async fn verify_fails_when_missing_headers() {
        let webhook = Webhook {
            headers: vec![],
            body: b"{}".to_vec(),
        };
        assert!(matches!(
            verify(&webhook, "k", now_unix()),
            Err(ProviderError::BadSignature)
        ));
    }

    /// SEC-05: a correctly signed request whose timestamp is older than
    /// the tolerance window must be rejected (replay).
    #[tokio::test]
    async fn verify_rejects_expired_timestamp() {
        const NOW: i64 = 1_716_355_200;
        let ts_millis = (NOW - TIMESTAMP_TOLERANCE_SECS - 1) * 1000;
        let webhook = signed_webhook_at(r#"{"msgtype":"text"}"#, "secret", ts_millis);
        assert!(matches!(
            verify(&webhook, "secret", NOW),
            Err(ProviderError::BadSignature)
        ));
    }

    /// SEC-05: a timestamp exactly at the tolerance boundary still passes.
    #[tokio::test]
    async fn verify_accepts_timestamp_within_window() {
        const NOW: i64 = 1_716_355_200;
        let ts_millis = (NOW - TIMESTAMP_TOLERANCE_SECS) * 1000;
        let webhook = signed_webhook_at(r#"{"msgtype":"text"}"#, "secret", ts_millis);
        verify(&webhook, "secret", NOW).expect("within window");
    }

    /// Validates the fallback query-string code path: when the gateway
    /// forwards the raw query as `x-dingtalk-query`, we URL-decode the
    /// `sign` parameter before comparing.
    #[tokio::test]
    async fn verify_accepts_url_encoded_query_form() {
        let now = now_unix();
        let ts = (now * 1000).to_string();
        let sig = sig_for(&ts, "secret");
        let encoded_sig = urlencoding::encode(&sig);
        let query = format!("timestamp={ts}&sign={encoded_sig}");
        let webhook = Webhook {
            headers: vec![("x-dingtalk-query".into(), query)],
            body: b"{\"msgtype\":\"text\"}".to_vec(),
        };
        verify(&webhook, "secret", now).expect("verify");
    }

    #[tokio::test]
    async fn parse_extracts_single_chat_fields() {
        let body = serde_json::to_string(&json!({
            "msgtype": "text",
            "text": {"content": "hello bot"},
            "conversationId": "cid_x",
            "conversationType": "1",
            "senderId": "u_temp",
            "senderStaffId": "u_real",
            "senderCorpId": "corp_x",
            "msgId": "m1"
        }))
        .unwrap();
        let webhook = signed_webhook(&body, "secret");
        let provider = DingTalkProvider::new("secret");
        match provider.parse(&webhook).await {
            Ok(ImEvent::Message(m)) => {
                assert_eq!(m.user_external_id, "u_real");
                assert_eq!(m.tenant_external_id, "corp_x");
                assert!(m.conversation_id.starts_with("dingtalk:single:"));
                assert!(m.conversation_id.contains("cid_x"));
                assert_eq!(m.text, "hello bot");
                assert_eq!(m.event_id, "m1");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn parse_distinguishes_group_chat() {
        let body = serde_json::to_string(&json!({
            "msgtype": "text",
            "text": {"content": "hi"},
            "conversationId": "cid_grp",
            "conversationType": "2",
            "senderStaffId": "u",
            "senderCorpId": "c",
            "msgId": "m"
        }))
        .unwrap();
        let webhook = signed_webhook(&body, "k");
        let provider = DingTalkProvider::new("k");
        let event = provider.parse(&webhook).await.unwrap();
        let ImEvent::Message(m) = event else {
            panic!("expected message");
        };
        assert!(m.conversation_id.starts_with("dingtalk:group:"));
        let (kind, conv, user) = split_conversation_id(&m.conversation_id).unwrap();
        assert_eq!(kind, ConversationKind::Group);
        assert_eq!(conv, "cid_grp");
        assert_eq!(user, "u");
    }

    #[tokio::test]
    async fn parse_rejects_non_text_msgtype() {
        let body = serde_json::to_string(&json!({
            "msgtype": "image",
            "conversationId": "x",
            "senderStaffId": "u",
            "senderCorpId": "c",
            "msgId": "m"
        }))
        .unwrap();
        let webhook = signed_webhook(&body, "k");
        let provider = DingTalkProvider::new("k");
        assert!(matches!(
            provider.parse(&webhook).await,
            Err(ProviderError::Malformed(_))
        ));
    }

    #[tokio::test]
    async fn reply_records_into_sink() {
        let sink = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let provider = DingTalkProvider::with_recording_sink("k", sink.clone());
        provider
            .reply(&OutgoingReply {
                conversation_id: "dingtalk:single:cid|user1".into(),
                text: "hi back".into(),
            })
            .await
            .unwrap();
        let buf = sink.lock();
        assert_eq!(buf.len(), 1);
        assert_eq!(buf[0].text, "hi back");
    }

    /// Exercises the API-sink path with a fake client. Proves the
    /// provider fetches a token, caches it across calls, picks the
    /// single-chat endpoint for `dingtalk:single:` and the group
    /// endpoint for `dingtalk:group:`, and forwards the right ids.
    #[tokio::test]
    async fn api_sink_routes_single_vs_group() {
        use parking_lot::Mutex as SyncMutex;
        use std::sync::Arc as StdArc;

        #[derive(Default)]
        struct Recorder {
            token_calls: SyncMutex<u32>,
            send_calls: SyncMutex<Vec<String>>,
        }

        #[async_trait::async_trait]
        impl DingTalkClient for Recorder {
            async fn fetch_access_token(
                &self,
                _app_key: &str,
                _app_secret: &str,
            ) -> Result<TokenResponse, ProviderError> {
                *self.token_calls.lock() += 1;
                Ok(TokenResponse {
                    token: "t".into(),
                    expire_in_secs: 7200,
                })
            }
            async fn send_single_text(
                &self,
                token: &str,
                robot: &str,
                users: &[String],
                text: &str,
            ) -> Result<JsonValue, ProviderError> {
                self.send_calls
                    .lock()
                    .push(format!("single|{token}|{robot}|{}|{text}", users.join(",")));
                Ok(json!({"ok": true}))
            }
            async fn send_group_text(
                &self,
                token: &str,
                robot: &str,
                conv: &str,
                text: &str,
            ) -> Result<JsonValue, ProviderError> {
                self.send_calls
                    .lock()
                    .push(format!("group|{token}|{robot}|{conv}|{text}"));
                Ok(json!({"ok": true}))
            }
        }

        let rec: StdArc<Recorder> = StdArc::new(Recorder::default());
        let provider = DingTalkProvider::with_api_sink(
            "webhook-secret",
            rec.clone() as StdArc<dyn DingTalkClient>,
            "app_key",
            "api_secret",
            "robot_xyz",
        );
        provider
            .reply(&OutgoingReply {
                conversation_id: "dingtalk:single:cid_a|user_a".into(),
                text: "hi single".into(),
            })
            .await
            .unwrap();
        provider
            .reply(&OutgoingReply {
                conversation_id: "dingtalk:group:cid_g|user_x".into(),
                text: "hi group".into(),
            })
            .await
            .unwrap();
        let calls = rec.send_calls.lock();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], "single|t|robot_xyz|user_a|hi single");
        assert_eq!(calls[1], "group|t|robot_xyz|cid_g|hi group");
        assert_eq!(*rec.token_calls.lock(), 1, "token cached across calls");
    }
}
