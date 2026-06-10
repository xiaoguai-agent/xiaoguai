//! Feishu IM adapter.
//!
//! v0.7 covers the inbound path: signature verification per Feishu's event
//! v2 spec + payload parsing into `xiaoguai_im_gateway::ImEvent`. Outbound
//! `reply(...)` is currently a stub — production Feishu `OpenAPI` bot reply
//! lands in v0.7.1 once `tenant_access_token` caching is in place.
//!
//! Signature shape Feishu uses (events v2, "Encrypt Key" enabled):
//! ```text
//! sig = sha256(timestamp + nonce + encrypt_key + body_str).hex
//! ```
//! delivered as the `X-Lark-Signature` header. We require:
//!   - `X-Lark-Request-Timestamp`
//!   - `X-Lark-Request-Nonce`
//!   - `X-Lark-Signature`
//!
//! Requests are rejected as `ProviderError::BadSignature` if anything
//! mismatches, or if `X-Lark-Request-Timestamp` falls outside the replay
//! window ([`TIMESTAMP_TOLERANCE_SECS`], SEC-05).

#![forbid(unsafe_code)]

pub mod api;

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use sha2::{Digest, Sha256};
use xiaoguai_im_gateway::{
    ImEvent, ImProvider, IncomingMessage, OutgoingReply, ProviderError, Webhook,
};

pub use api::{FeishuClient, HttpFeishuClient, TokenCache, TokenResponse, DEFAULT_BASE_URL};

/// SEC-05/SEC-12: maximum clock skew (seconds) allowed between
/// `X-Lark-Request-Timestamp` and the current wall clock — the replay
/// window. Mirrors the Slack adapter's 5-minute tolerance.
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
pub struct FeishuProvider {
    encrypt_key: String,
    // Outbound is a stub in v0.7. v0.7.1 will hold a Feishu OpenAPI client.
    reply_sink: ReplySink,
}

#[derive(Clone, Default)]
pub enum ReplySink {
    /// Discard the reply. Used in tests + dev.
    #[default]
    Stub,
    /// In-memory recorder so tests can assert on what would have been sent.
    Recording(std::sync::Arc<parking_lot::Mutex<Vec<OutgoingReply>>>),
    /// Send via the real Feishu `OpenAPI`. v0.7.1+.
    Api(Arc<ApiSink>),
}

/// Holds the HTTP client + token cache for the real-`OpenAPI` reply path.
pub struct ApiSink {
    pub client: Arc<dyn FeishuClient>,
    pub token_cache: TokenCache,
}

impl ApiSink {
    #[must_use]
    pub fn new(client: Arc<dyn FeishuClient>, app_id: String, app_secret: String) -> Self {
        let token_cache = TokenCache::new(Arc::clone(&client), app_id, app_secret);
        Self {
            client,
            token_cache,
        }
    }
}

impl FeishuProvider {
    #[must_use]
    pub fn new(encrypt_key: impl Into<String>) -> Self {
        Self {
            encrypt_key: encrypt_key.into(),
            reply_sink: ReplySink::Stub,
        }
    }

    #[must_use]
    pub fn with_recording_sink(
        encrypt_key: impl Into<String>,
        sink: std::sync::Arc<parking_lot::Mutex<Vec<OutgoingReply>>>,
    ) -> Self {
        Self {
            encrypt_key: encrypt_key.into(),
            reply_sink: ReplySink::Recording(sink),
        }
    }

    /// Build a provider that sends replies through the real Feishu
    /// `OpenAPI` (`im/v1/messages`). The `client` parameter is generic
    /// over [`FeishuClient`] so tests can drive the full provider
    /// without hitting the network.
    #[must_use]
    pub fn with_api_sink(
        encrypt_key: impl Into<String>,
        client: Arc<dyn FeishuClient>,
        app_id: impl Into<String>,
        app_secret: impl Into<String>,
    ) -> Self {
        Self {
            encrypt_key: encrypt_key.into(),
            reply_sink: ReplySink::Api(Arc::new(ApiSink::new(
                client,
                app_id.into(),
                app_secret.into(),
            ))),
        }
    }
}

/// Verify a Feishu signature per the events v2 spec.
///
/// Returns Ok when all three pieces (`timestamp`, `nonce`, `signature`)
/// are present, the timestamp is within ±[`TIMESTAMP_TOLERANCE_SECS`] of
/// `now_unix`, and reconstructing the digest matches.
///
/// `now_unix` is the current Unix timestamp in seconds; pass
/// [`now_unix()`] in production and a fixed value in tests.
fn verify(webhook: &Webhook, encrypt_key: &str, now_unix: i64) -> Result<(), ProviderError> {
    let timestamp = webhook
        .header("X-Lark-Request-Timestamp")
        .ok_or(ProviderError::BadSignature)?;
    let nonce = webhook
        .header("X-Lark-Request-Nonce")
        .ok_or(ProviderError::BadSignature)?;
    let signature = webhook
        .header("X-Lark-Signature")
        .ok_or(ProviderError::BadSignature)?;

    // SEC-05/SEC-12 replay protection: the Feishu timestamp is seconds
    // since the Unix epoch. Unparsable or stale timestamps are rejected
    // (fail-closed).
    let ts: i64 = timestamp.parse().map_err(|_| ProviderError::BadSignature)?;
    if (ts - now_unix).abs() > TIMESTAMP_TOLERANCE_SECS {
        return Err(ProviderError::BadSignature);
    }

    let mut hasher = Sha256::new();
    hasher.update(timestamp.as_bytes());
    hasher.update(nonce.as_bytes());
    hasher.update(encrypt_key.as_bytes());
    hasher.update(&webhook.body);
    let computed = hex::encode(hasher.finalize());

    // Constant-time compare avoids signal leakage on partial mismatches.
    if constant_time_eq(computed.as_bytes(), signature.as_bytes()) {
        Ok(())
    } else {
        Err(ProviderError::BadSignature)
    }
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
struct OuterEnvelope {
    #[serde(default)]
    challenge: Option<String>,
    #[serde(default)]
    header: Option<FeishuHeader>,
    #[serde(default)]
    event: Option<FeishuEventBody>,
}

#[derive(Deserialize)]
struct FeishuHeader {
    event_id: String,
    #[serde(default)]
    tenant_key: Option<String>,
}

#[derive(Deserialize)]
struct FeishuEventBody {
    #[serde(default)]
    sender: Option<FeishuSender>,
    #[serde(default)]
    message: Option<FeishuMessage>,
}

#[derive(Deserialize)]
struct FeishuSender {
    #[serde(default)]
    sender_id: Option<FeishuSenderId>,
    #[serde(default)]
    tenant_key: Option<String>,
}

#[derive(Deserialize)]
struct FeishuSenderId {
    #[serde(default)]
    open_id: Option<String>,
}

#[derive(Deserialize)]
struct FeishuMessage {
    chat_id: String,
    #[serde(default)]
    content: Option<String>,
}

fn parse_event(body: &[u8]) -> Result<ImEvent, ProviderError> {
    let envelope: OuterEnvelope = serde_json::from_slice(body)
        .map_err(|e| ProviderError::Malformed(format!("decode envelope: {e}")))?;

    if let Some(challenge) = envelope.challenge {
        return Ok(ImEvent::Challenge { challenge });
    }

    let header = envelope
        .header
        .ok_or_else(|| ProviderError::Malformed("missing header".into()))?;
    let event = envelope
        .event
        .ok_or_else(|| ProviderError::Malformed("missing event".into()))?;
    let sender = event
        .sender
        .ok_or_else(|| ProviderError::Malformed("missing sender".into()))?;
    let message = event
        .message
        .ok_or_else(|| ProviderError::Malformed("missing message".into()))?;
    let user_open_id = sender
        .sender_id
        .and_then(|sid| sid.open_id)
        .ok_or_else(|| ProviderError::Malformed("missing sender open_id".into()))?;
    let tenant_key = sender
        .tenant_key
        .or(header.tenant_key)
        .ok_or_else(|| ProviderError::Malformed("missing tenant_key".into()))?;

    // Feishu wraps text-mode messages as `{"text":"hello"}` JSON.
    let text = if let Some(raw) = message.content {
        match serde_json::from_str::<JsonValue>(&raw) {
            Ok(v) => v
                .get("text")
                .and_then(JsonValue::as_str)
                .unwrap_or(&raw)
                .to_string(),
            Err(_) => raw,
        }
    } else {
        String::new()
    };

    Ok(ImEvent::Message(IncomingMessage {
        provider: "feishu".into(),
        user_external_id: user_open_id,
        tenant_external_id: tenant_key,
        conversation_id: message.chat_id,
        text,
        event_id: header.event_id,
    }))
}

#[async_trait]
impl ImProvider for FeishuProvider {
    async fn parse(&self, webhook: &Webhook) -> Result<ImEvent, ProviderError> {
        // Challenge requests still need signing per Feishu's spec — verify
        // *before* peeking at the body so unauthenticated peers can't
        // probe the challenge path. SEC-05: this is the only call-site
        // that reads the system clock; `verify` stays clock-injectable.
        verify(webhook, &self.encrypt_key, now_unix())?;
        parse_event(&webhook.body)
    }

    async fn reply(&self, out: &OutgoingReply) -> Result<JsonValue, ProviderError> {
        match &self.reply_sink {
            ReplySink::Stub => {
                tracing::debug!(?out, "feishu reply stub");
                Ok(json!({"status":"stubbed"}))
            }
            ReplySink::Recording(buf) => {
                buf.lock().push(out.clone());
                Ok(json!({"status":"recorded"}))
            }
            ReplySink::Api(sink) => {
                let token = sink.token_cache.get_token().await?;
                let resp = sink
                    .client
                    .send_text_message(&token, &out.conversation_id, &out.text)
                    .await?;
                tracing::info!(chat_id = %out.conversation_id, "feishu reply sent");
                Ok(resp)
            }
        }
    }

    fn name(&self) -> &'static str {
        "feishu"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a webhook signed for an explicit Unix-second timestamp so
    /// freshness tests can place it precisely relative to `now`.
    fn signed_webhook_at(body: &str, encrypt_key: &str, ts_secs: i64) -> Webhook {
        let ts = ts_secs.to_string();
        let nonce = "abc123";
        let mut hasher = Sha256::new();
        hasher.update(ts.as_bytes());
        hasher.update(nonce.as_bytes());
        hasher.update(encrypt_key.as_bytes());
        hasher.update(body.as_bytes());
        let sig = hex::encode(hasher.finalize());
        Webhook {
            headers: vec![
                ("X-Lark-Request-Timestamp".into(), ts),
                ("X-Lark-Request-Nonce".into(), nonce.into()),
                ("X-Lark-Signature".into(), sig),
            ],
            body: body.as_bytes().to_vec(),
        }
    }

    /// Webhook signed "now" — survives the SEC-05 freshness window when
    /// the full `provider.parse` path reads the real clock.
    fn signed_webhook(body: &str, encrypt_key: &str) -> Webhook {
        signed_webhook_at(body, encrypt_key, now_unix())
    }

    #[tokio::test]
    async fn verify_passes_with_matching_signature() {
        let webhook = signed_webhook(r#"{"challenge":"x"}"#, "secret");
        verify(&webhook, "secret", now_unix()).expect("verify");
    }

    #[tokio::test]
    async fn verify_fails_with_wrong_secret() {
        let webhook = signed_webhook(r#"{"challenge":"x"}"#, "secret");
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
        let webhook = signed_webhook_at(
            r#"{"challenge":"x"}"#,
            "secret",
            NOW - TIMESTAMP_TOLERANCE_SECS - 1,
        );
        assert!(matches!(
            verify(&webhook, "secret", NOW),
            Err(ProviderError::BadSignature)
        ));
    }

    /// SEC-05: a timestamp exactly at the tolerance boundary still passes.
    #[tokio::test]
    async fn verify_accepts_timestamp_within_window() {
        const NOW: i64 = 1_716_355_200;
        let webhook = signed_webhook_at(
            r#"{"challenge":"x"}"#,
            "secret",
            NOW - TIMESTAMP_TOLERANCE_SECS,
        );
        verify(&webhook, "secret", NOW).expect("within window");
    }

    #[tokio::test]
    async fn challenge_path_round_trips() {
        let body = r#"{"challenge":"hello-challenge"}"#;
        let webhook = signed_webhook(body, "k");
        let provider = FeishuProvider::new("k");
        match provider.parse(&webhook).await {
            Ok(ImEvent::Challenge { challenge }) => assert_eq!(challenge, "hello-challenge"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn message_path_parses_sender_and_text() {
        let body = serde_json::to_string(&json!({
            "header": {"event_id": "evt-1", "tenant_key": "ten_x"},
            "event": {
                "sender": {"sender_id": {"open_id": "ou_alice"}, "tenant_key": "ten_x"},
                "message": {"chat_id": "oc_chat", "content": "{\"text\":\"hi\"}"}
            }
        }))
        .unwrap();
        let webhook = signed_webhook(&body, "k");
        let provider = FeishuProvider::new("k");
        match provider.parse(&webhook).await {
            Ok(ImEvent::Message(m)) => {
                assert_eq!(m.user_external_id, "ou_alice");
                assert_eq!(m.tenant_external_id, "ten_x");
                assert_eq!(m.conversation_id, "oc_chat");
                assert_eq!(m.text, "hi");
                assert_eq!(m.event_id, "evt-1");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn reply_records_into_sink() {
        let sink = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let provider = FeishuProvider::with_recording_sink("k", sink.clone());
        provider
            .reply(&OutgoingReply {
                conversation_id: "oc_x".into(),
                text: "hi back".into(),
            })
            .await
            .unwrap();
        let buf = sink.lock();
        assert_eq!(buf.len(), 1);
        assert_eq!(buf[0].text, "hi back");
    }

    /// Exercises the full Api-sink path with a fake `FeishuClient` that
    /// records every outbound call. Proves the provider:
    ///   - fetches a token through the cache,
    ///   - uses the cached token on subsequent replies,
    ///   - forwards `chat_id` + `text` to `send_text_message`.
    #[tokio::test]
    async fn api_sink_calls_through_to_client() {
        use parking_lot::Mutex as SyncMutex;
        use std::sync::Arc as StdArc;

        #[derive(Default)]
        struct Recorder {
            token_calls: SyncMutex<u32>,
            send_calls: SyncMutex<Vec<(String, String, String)>>,
        }

        #[async_trait::async_trait]
        impl FeishuClient for Recorder {
            async fn fetch_tenant_access_token(
                &self,
                _app_id: &str,
                _app_secret: &str,
            ) -> Result<TokenResponse, ProviderError> {
                *self.token_calls.lock() += 1;
                Ok(TokenResponse {
                    token: "t_real".into(),
                    expire_in_secs: 7200,
                })
            }
            async fn send_text_message(
                &self,
                token: &str,
                chat_id: &str,
                text: &str,
            ) -> Result<JsonValue, ProviderError> {
                self.send_calls.lock().push((
                    token.to_string(),
                    chat_id.to_string(),
                    text.to_string(),
                ));
                Ok(json!({"code": 0}))
            }
        }

        let rec: StdArc<Recorder> = StdArc::new(Recorder::default());
        let provider = FeishuProvider::with_api_sink(
            "encrypt-key",
            rec.clone() as StdArc<dyn FeishuClient>,
            "cli_app",
            "secret",
        );
        provider
            .reply(&OutgoingReply {
                conversation_id: "oc_a".into(),
                text: "first reply".into(),
            })
            .await
            .unwrap();
        provider
            .reply(&OutgoingReply {
                conversation_id: "oc_a".into(),
                text: "second reply".into(),
            })
            .await
            .unwrap();
        let calls = rec.send_calls.lock();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0],
            ("t_real".into(), "oc_a".into(), "first reply".into())
        );
        assert_eq!(calls[1].2, "second reply");
        assert_eq!(*rec.token_calls.lock(), 1, "token should be cached");
    }
}
