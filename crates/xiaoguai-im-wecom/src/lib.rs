//! WeCom (企业微信) IM adapter.
//!
//! v1.1.3 ships the inbound + outbound path mirroring `xiaoguai-im-feishu`:
//! webhook signature verification, XML payload parsing into
//! `xiaoguai_im_gateway::ImEvent`, and an `OpenAPI` reply sink with
//! `access_token` caching.
//!
//! ## Signature
//! ```text
//! sig = sha1(sort([token, timestamp, nonce, msg_encrypt]).join(""))
//! ```
//! The four inputs are sorted lexicographically, concatenated as plain
//! strings, hashed with SHA-1, and the lower-case hex digest is sent in
//! the `msg_signature` query parameter. The adapter accepts those four
//! query fields either via dedicated headers (preferred — what the
//! gateway should be wiring), or via an `x-wecom-query` URL-encoded
//! query-string fallback.
//!
//! ## URL verification
//! WeCom's URL verification step pings the endpoint with `echostr` and
//! expects the **decrypted** plain text returned verbatim. For the
//! v1.1.3 plain-text mode (encryption disabled on the WeCom side), the
//! adapter treats `echostr` as the literal challenge token; the actual
//! AES-decrypt + echo path is deferred (see the v1.1.3 plan doc).
//!
//! ## Payload
//! WeCom delivers XML even when encryption is disabled:
//! ```xml
//! <xml>
//!   <ToUserName>corp_id</ToUserName>
//!   <FromUserName>user_id</FromUserName>
//!   <CreateTime>1716355200</CreateTime>
//!   <MsgType>text</MsgType>
//!   <Content>hello</Content>
//!   <MsgId>123456</MsgId>
//!   <AgentID>1000002</AgentID>
//! </xml>
//! ```
//!
//! Encrypted variant: the body holds `<Encrypt>...</Encrypt>` whose
//! contents are AES-CBC-256 ciphertext keyed off `EncodingAESKey`. We
//! detect the encrypted shape and reject it as `Malformed` with a
//! direction to disable encryption (v1.1.3 plain-text only). Full AES
//! decrypt is deferred to a follow-up tag.

#![forbid(unsafe_code)]

pub mod api;
pub mod crypto;

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use sha1::{Digest, Sha1};
use xiaoguai_im_gateway::{
    ImEvent, ImProvider, IncomingMessage, OutgoingReply, ProviderError, Webhook,
};

pub use api::{HttpWeComClient, TokenCache, TokenResponse, WeComClient, DEFAULT_BASE_URL};
pub use crypto::{WecomCrypto, WecomCryptoError};

#[derive(Clone)]
pub struct WeComProvider {
    /// Inbound signing token (`Token` in WeCom callback config).
    token: String,
    reply_sink: ReplySink,
    /// Optional AES crypto for encrypted-callback mode.
    /// When `None`, the adapter operates in plain-text mode (legacy /
    /// deployments with encryption disabled on the WeCom side).
    crypto: Option<std::sync::Arc<WecomCrypto>>,
}

#[derive(Clone, Default)]
pub enum ReplySink {
    /// Discard the reply. Used in tests + dev.
    #[default]
    Stub,
    /// In-memory recorder so tests can assert on what would have been sent.
    Recording(std::sync::Arc<parking_lot::Mutex<Vec<OutgoingReply>>>),
    /// Send via the real WeCom `OpenAPI`.
    Api(Arc<ApiSink>),
}

/// Holds the HTTP client + token cache + agent identity for the
/// real-`OpenAPI` reply path.
pub struct ApiSink {
    pub client: Arc<dyn WeComClient>,
    pub token_cache: TokenCache,
    /// WeCom `agentid` — required by `message/send`.
    pub agent_id: i64,
}

impl ApiSink {
    #[must_use]
    pub fn new(
        client: Arc<dyn WeComClient>,
        corp_id: String,
        corp_secret: String,
        agent_id: i64,
    ) -> Self {
        let token_cache = TokenCache::new(Arc::clone(&client), corp_id, corp_secret);
        Self {
            client,
            token_cache,
            agent_id,
        }
    }
}

impl WeComProvider {
    /// Create a provider for **plain-text** callbacks (encryption disabled on
    /// the WeCom console).
    #[must_use]
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            reply_sink: ReplySink::Stub,
            crypto: None,
        }
    }

    /// Attach AES crypto to an existing provider (builder-style).
    ///
    /// Once set, `msg_signature` in the inbound request triggers the
    /// encrypted path; plain-text inbound still works when `msg_signature`
    /// is absent.
    #[must_use]
    pub fn with_crypto(mut self, crypto: WecomCrypto) -> Self {
        self.crypto = Some(std::sync::Arc::new(crypto));
        self
    }

    #[must_use]
    pub fn with_recording_sink(
        token: impl Into<String>,
        sink: std::sync::Arc<parking_lot::Mutex<Vec<OutgoingReply>>>,
    ) -> Self {
        Self {
            token: token.into(),
            reply_sink: ReplySink::Recording(sink),
            crypto: None,
        }
    }

    #[must_use]
    pub fn with_api_sink(
        token: impl Into<String>,
        client: Arc<dyn WeComClient>,
        corp_id: impl Into<String>,
        corp_secret: impl Into<String>,
        agent_id: i64,
    ) -> Self {
        Self {
            token: token.into(),
            reply_sink: ReplySink::Api(Arc::new(ApiSink::new(
                client,
                corp_id.into(),
                corp_secret.into(),
                agent_id,
            ))),
            crypto: None,
        }
    }
}

/// Verify a WeCom webhook signature.
///
/// Reads `timestamp` + `nonce` + `msg_signature` from one of:
///   - explicit headers `X-WeCom-Timestamp` + `X-WeCom-Nonce` +
///     `X-WeCom-Msg-Signature` (preferred), or
///   - the `x-wecom-query` header (URL-encoded query string) for
///     transports that don't expose individual query keys.
///
/// `body_text` is what goes into the signature: for encrypted-mode
/// requests it's the `Encrypt` element's contents, for plain-text mode
/// (URL verification) it's the `echostr` value. The caller passes
/// whichever applies.
fn verify(webhook: &Webhook, token: &str, body_text: &str) -> Result<(), ProviderError> {
    let (timestamp, nonce, signature) =
        read_sig_triple(webhook).ok_or(ProviderError::BadSignature)?;

    let mut parts = [token, &timestamp, &nonce, body_text];
    parts.sort_unstable();
    let joined: String = parts.concat();
    let mut hasher = Sha1::new();
    hasher.update(joined.as_bytes());
    let computed = hex_lower(&hasher.finalize());

    if constant_time_eq(computed.as_bytes(), signature.as_bytes()) {
        Ok(())
    } else {
        Err(ProviderError::BadSignature)
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

/// Extract `(timestamp, nonce, msg_signature)`. Tries headers first,
/// then the `x-wecom-query` fallback.
fn read_sig_triple(webhook: &Webhook) -> Option<(String, String, String)> {
    if let (Some(ts), Some(nonce), Some(sig)) = (
        webhook.header("X-WeCom-Timestamp"),
        webhook.header("X-WeCom-Nonce"),
        webhook.header("X-WeCom-Msg-Signature"),
    ) {
        return Some((ts.to_string(), nonce.to_string(), sig.to_string()));
    }
    let query = webhook.header("x-wecom-query")?;
    let mut ts: Option<String> = None;
    let mut nonce: Option<String> = None;
    let mut sig: Option<String> = None;
    for pair in query.split('&') {
        let (key, val) = pair.split_once('=')?;
        // WeCom doesn't URL-encode these (they're decimal/hex strings)
        // but we still defensively decode in case a proxy did.
        let decoded = percent_decode(val);
        match key {
            "timestamp" => ts = Some(decoded),
            "nonce" => nonce = Some(decoded),
            "msg_signature" => sig = Some(decoded),
            _ => {}
        }
    }
    Some((ts?, nonce?, sig?))
}

fn percent_decode(s: &str) -> String {
    // Minimal — no allocations beyond the result.
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2])) {
                out.push((hi * 16 + lo) as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(10 + b - b'a'),
        b'A'..=b'F' => Some(10 + b - b'A'),
        _ => None,
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

/// XML envelope WeCom delivers. Only the fields we consume are
/// declared; unknown fields are ignored by quick-xml.
#[derive(Debug, Deserialize, Default)]
#[serde(rename = "xml")]
struct WeComXml {
    #[serde(default, rename = "Encrypt")]
    encrypt: Option<String>,
    #[serde(default, rename = "ToUserName")]
    to_user_name: Option<String>,
    #[serde(default, rename = "FromUserName")]
    from_user_name: Option<String>,
    #[serde(default, rename = "MsgType")]
    msg_type: Option<String>,
    #[serde(default, rename = "Content")]
    content: Option<String>,
    #[serde(default, rename = "MsgId")]
    msg_id: Option<String>,
    // Note: AgentID is present in the inbound XML but unused — the
    // outbound `agent_id` comes from operator config (the WeCom app
    // can only reply as itself).
}

/// Parse the WeCom XML body, distinguishing URL-verification (echostr
/// is in the query string, not the body) from a real text-message
/// payload. The challenge path is **not** triggered from XML — it
/// arrives as a `GET` whose `echostr` query value the gateway hands us
/// in `body` directly. Callers may bypass parse and route the GET
/// straight to the [`echo_challenge`] helper.
fn parse_event(body: &[u8]) -> Result<ImEvent, ProviderError> {
    let body_str = std::str::from_utf8(body)
        .map_err(|e| ProviderError::Malformed(format!("body not utf8: {e}")))?;
    let xml: WeComXml = quick_xml::de::from_str(body_str)
        .map_err(|e| ProviderError::Malformed(format!("decode xml: {e}")))?;

    // Note: `<Encrypt>` at this point means the caller has already decrypted
    // the outer envelope and we are parsing the *inner* XML — `encrypt` should
    // be absent here.  If somehow a caller passes an undecrypted envelope, we
    // produce a clear error rather than silently discarding it.
    if xml.encrypt.is_some() {
        return Err(ProviderError::Malformed(
            "parse_event called with undecrypted envelope; \
             the provider should decrypt before calling parse_event"
                .into(),
        ));
    }

    match xml.msg_type.as_deref() {
        Some("text") => {}
        Some(other) => {
            return Err(ProviderError::Malformed(format!(
                "unsupported MsgType: {other}"
            )))
        }
        None => return Err(ProviderError::Malformed("missing MsgType".into())),
    }

    let conversation_id = xml
        .to_user_name
        .clone()
        .ok_or_else(|| ProviderError::Malformed("missing ToUserName".into()))?;
    let user_external_id = xml
        .from_user_name
        .ok_or_else(|| ProviderError::Malformed("missing FromUserName".into()))?;
    let tenant_external_id = xml
        .to_user_name
        .ok_or_else(|| ProviderError::Malformed("missing ToUserName".into()))?;
    let event_id = xml
        .msg_id
        .unwrap_or_else(|| format!("wecom-{}", chrono::Utc::now().timestamp_millis()));
    let text = xml.content.unwrap_or_default();
    // We encode `(corp_id, user_id)` into the conversation_id so the
    // reply path can route back through `message/send` to the right
    // `touser`. WeCom inbound has no notion of group chat for this
    // endpoint — each text is a 1:1 user → app conversation.
    let conversation_id = format!("wecom:{conversation_id}|{user_external_id}");

    Ok(ImEvent::Message(IncomingMessage {
        provider: "wecom".into(),
        user_external_id,
        tenant_external_id,
        conversation_id,
        text,
        event_id,
    }))
}

/// Compute the response for a URL-verification ping.
///
/// WeCom's verification step issues a `GET` with `echostr` as a query
/// parameter; the server must echo it (decrypted, when encryption is
/// enabled). v1.1.3 supports plain-text mode only — the gateway hands
/// us the `echostr` and we return it verbatim after signature checks.
pub fn echo_challenge(
    webhook: &Webhook,
    token: &str,
    echostr: &str,
) -> Result<String, ProviderError> {
    verify(webhook, token, echostr)?;
    Ok(echostr.to_string())
}

/// Decode the `wecom:<corp_id>|<user_id>` conversation-id encoding back
/// into its parts.
#[must_use]
pub fn split_conversation_id(id: &str) -> Option<(&str, &str)> {
    let rest = id.strip_prefix("wecom:")?;
    let (conv, user) = rest.split_once('|')?;
    Some((conv, user))
}

#[async_trait]
impl ImProvider for WeComProvider {
    async fn parse(&self, webhook: &Webhook) -> Result<ImEvent, ProviderError> {
        let body_text = std::str::from_utf8(&webhook.body)
            .map_err(|e| ProviderError::Malformed(format!("body not utf8: {e}")))?;

        // Parse the XML envelope first so we can detect which mode we're in.
        // Encrypted bodies contain <Encrypt> while plain-text bodies contain
        // <MsgType>/<Content> directly.  Both modes use msg_signature for the
        // signature header, so we cannot use that as the discriminator.
        let xml: WeComXml = quick_xml::de::from_str(body_text)
            .map_err(|e| ProviderError::Malformed(format!("decode xml: {e}")))?;

        if let Some(encrypt_blob) = xml.encrypt {
            // ── Encrypted path ──────────────────────────────────────────────
            let crypto = self.crypto.as_ref().ok_or_else(|| {
                ProviderError::Malformed(
                    "wecom encrypted payload received but EncodingAESKey not configured; \
                     call WeComProvider::with_crypto() or disable encryption in the WeCom console"
                        .into(),
                )
            })?;

            // Verify signature (signed over the Encrypt blob value).
            let (timestamp, nonce, signature) =
                read_sig_triple(webhook).ok_or(ProviderError::BadSignature)?;
            if !crypto.verify_signature(&signature, &timestamp, &nonce, &encrypt_blob) {
                return Err(ProviderError::BadSignature);
            }

            // Decrypt → inner XML.
            let inner_xml = crypto
                .decrypt(&encrypt_blob)
                .map_err(|e| ProviderError::Malformed(format!("aes decrypt: {e}")))?;
            parse_event(inner_xml.as_bytes())
        } else {
            // ── Plain-text path (backward-compatible) ──────────────────────
            // For plain-text, the signature is over the full body string.
            verify(webhook, &self.token, body_text)?;
            parse_event(&webhook.body)
        }
    }

    async fn reply(&self, out: &OutgoingReply) -> Result<JsonValue, ProviderError> {
        match &self.reply_sink {
            ReplySink::Stub => {
                tracing::debug!(?out, "wecom reply stub");
                Ok(json!({"status":"stubbed"}))
            }
            ReplySink::Recording(buf) => {
                buf.lock().push(out.clone());
                Ok(json!({"status":"recorded"}))
            }
            ReplySink::Api(sink) => {
                let token = sink.token_cache.get_token().await?;
                let (_conv, user) =
                    split_conversation_id(&out.conversation_id).ok_or_else(|| {
                        ProviderError::Transport(format!(
                            "unrecognised wecom conversation_id format: {}",
                            out.conversation_id
                        ))
                    })?;
                let resp = sink
                    .client
                    .send_text(&token, sink.agent_id, user, &out.text)
                    .await?;
                tracing::info!(touser = %user, "wecom reply sent");
                Ok(resp)
            }
        }
    }

    fn name(&self) -> &'static str {
        "wecom"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig_for(token: &str, ts: &str, nonce: &str, body_text: &str) -> String {
        let mut parts = [token, ts, nonce, body_text];
        parts.sort_unstable();
        let mut hasher = Sha1::new();
        hasher.update(parts.concat().as_bytes());
        hex_lower(&hasher.finalize())
    }

    fn signed_webhook(body: &str, token: &str) -> Webhook {
        let ts = "1716355200";
        let nonce = "nonce_x";
        let sig = sig_for(token, ts, nonce, body);
        Webhook {
            headers: vec![
                ("X-WeCom-Timestamp".into(), ts.into()),
                ("X-WeCom-Nonce".into(), nonce.into()),
                ("X-WeCom-Msg-Signature".into(), sig),
            ],
            body: body.as_bytes().to_vec(),
        }
    }

    #[tokio::test]
    async fn verify_passes_with_matching_signature() {
        let body = "<xml></xml>";
        let webhook = signed_webhook(body, "tok");
        verify(&webhook, "tok", body).expect("verify");
    }

    #[tokio::test]
    async fn verify_fails_with_wrong_token() {
        let body = "<xml></xml>";
        let webhook = signed_webhook(body, "tok");
        assert!(matches!(
            verify(&webhook, "other-tok", body),
            Err(ProviderError::BadSignature)
        ));
    }

    #[tokio::test]
    async fn verify_fails_when_body_tampered() {
        let body = "<xml></xml>";
        let webhook = signed_webhook(body, "tok");
        assert!(matches!(
            verify(&webhook, "tok", "<xml>tampered</xml>"),
            Err(ProviderError::BadSignature)
        ));
    }

    #[tokio::test]
    async fn verify_accepts_query_string_form() {
        let ts = "1716355200";
        let nonce = "nonce_x";
        let body = "<xml></xml>";
        let sig = sig_for("tok", ts, nonce, body);
        let query = format!("timestamp={ts}&nonce={nonce}&msg_signature={sig}");
        let webhook = Webhook {
            headers: vec![("x-wecom-query".into(), query)],
            body: body.as_bytes().to_vec(),
        };
        verify(&webhook, "tok", body).expect("verify");
    }

    #[tokio::test]
    async fn parse_extracts_text_message_fields() {
        let body = r"<xml>
            <ToUserName>corp_x</ToUserName>
            <FromUserName>user_a</FromUserName>
            <CreateTime>1716355200</CreateTime>
            <MsgType>text</MsgType>
            <Content>hi bot</Content>
            <MsgId>123</MsgId>
            <AgentID>1000002</AgentID>
        </xml>";
        let webhook = signed_webhook(body, "tok");
        let provider = WeComProvider::new("tok");
        match provider.parse(&webhook).await {
            Ok(ImEvent::Message(m)) => {
                assert_eq!(m.user_external_id, "user_a");
                assert_eq!(m.tenant_external_id, "corp_x");
                assert!(m.conversation_id.starts_with("wecom:corp_x|user_a"));
                assert_eq!(m.text, "hi bot");
                assert_eq!(m.event_id, "123");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn parse_rejects_encrypted_payload_without_crypto_configured() {
        // The body has an <Encrypt> element AND the webhook includes
        // X-WeCom-Msg-Signature, triggering the encrypted path. Because
        // `WeComProvider::new` has no crypto configured, it should
        // return a Malformed error directing the operator to call
        // `with_crypto()` or disable encryption.
        let ts = "1716355200";
        let nonce = "nonce_x";
        let encrypt_blob = "base64-blob";
        let sig = sig_for("tok", ts, nonce, encrypt_blob);
        let body =
            format!("<xml><Encrypt>{encrypt_blob}</Encrypt><ToUserName>corp_x</ToUserName></xml>");
        let webhook = Webhook {
            headers: vec![
                ("X-WeCom-Timestamp".into(), ts.into()),
                ("X-WeCom-Nonce".into(), nonce.into()),
                ("X-WeCom-Msg-Signature".into(), sig),
            ],
            body: body.as_bytes().to_vec(),
        };
        let provider = WeComProvider::new("tok");
        let err = provider.parse(&webhook).await.unwrap_err();
        match err {
            ProviderError::Malformed(msg) => {
                assert!(
                    msg.contains("EncodingAESKey not configured"),
                    "unexpected msg: {msg}"
                );
            }
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn parse_rejects_non_text_msgtype() {
        let body = r"<xml>
            <ToUserName>c</ToUserName>
            <FromUserName>u</FromUserName>
            <MsgType>image</MsgType>
        </xml>";
        let webhook = signed_webhook(body, "tok");
        let provider = WeComProvider::new("tok");
        assert!(matches!(
            provider.parse(&webhook).await,
            Err(ProviderError::Malformed(_))
        ));
    }

    /// URL verification: the gateway hands us the `echostr` from the
    /// query string, we verify the signature with `echostr` as the
    /// signed body, and return it verbatim (plain-text mode).
    #[tokio::test]
    async fn echo_challenge_round_trips() {
        let ts = "1716355200";
        let nonce = "nonce_x";
        let echostr = "verify-me";
        let sig = sig_for("tok", ts, nonce, echostr);
        let webhook = Webhook {
            headers: vec![
                ("X-WeCom-Timestamp".into(), ts.into()),
                ("X-WeCom-Nonce".into(), nonce.into()),
                ("X-WeCom-Msg-Signature".into(), sig),
            ],
            body: vec![],
        };
        let echoed = echo_challenge(&webhook, "tok", echostr).expect("echo ok");
        assert_eq!(echoed, "verify-me");
    }

    #[tokio::test]
    async fn echo_challenge_rejects_bad_signature() {
        let webhook = Webhook {
            headers: vec![
                ("X-WeCom-Timestamp".into(), "1".into()),
                ("X-WeCom-Nonce".into(), "n".into()),
                ("X-WeCom-Msg-Signature".into(), "deadbeef".into()),
            ],
            body: vec![],
        };
        assert!(matches!(
            echo_challenge(&webhook, "tok", "verify-me"),
            Err(ProviderError::BadSignature)
        ));
    }

    #[tokio::test]
    async fn reply_records_into_sink() {
        let sink = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
        let provider = WeComProvider::with_recording_sink("tok", sink.clone());
        provider
            .reply(&OutgoingReply {
                conversation_id: "wecom:corp_x|user_a".into(),
                text: "hi back".into(),
            })
            .await
            .unwrap();
        let buf = sink.lock();
        assert_eq!(buf.len(), 1);
        assert_eq!(buf[0].text, "hi back");
    }

    /// API-sink round-trip: caching + correct `touser` extraction +
    /// constant-`agent_id` forwarding.
    #[tokio::test]
    async fn api_sink_routes_to_send_text() {
        use parking_lot::Mutex as SyncMutex;
        use std::sync::Arc as StdArc;

        #[derive(Default)]
        struct Recorder {
            token_calls: SyncMutex<u32>,
            send_calls: SyncMutex<Vec<String>>,
        }

        #[async_trait::async_trait]
        impl WeComClient for Recorder {
            async fn fetch_access_token(
                &self,
                _corp_id: &str,
                _corp_secret: &str,
            ) -> Result<TokenResponse, ProviderError> {
                *self.token_calls.lock() += 1;
                Ok(TokenResponse {
                    token: "t".into(),
                    expire_in_secs: 7200,
                })
            }
            async fn send_text(
                &self,
                token: &str,
                agent_id: i64,
                touser: &str,
                text: &str,
            ) -> Result<JsonValue, ProviderError> {
                self.send_calls
                    .lock()
                    .push(format!("{token}|{agent_id}|{touser}|{text}"));
                Ok(json!({"errcode": 0}))
            }
        }

        let rec: StdArc<Recorder> = StdArc::new(Recorder::default());
        let provider = WeComProvider::with_api_sink(
            "callback-token",
            rec.clone() as StdArc<dyn WeComClient>,
            "corp_x",
            "sec_x",
            1_000_002,
        );
        provider
            .reply(&OutgoingReply {
                conversation_id: "wecom:corp_x|user_a".into(),
                text: "first".into(),
            })
            .await
            .unwrap();
        provider
            .reply(&OutgoingReply {
                conversation_id: "wecom:corp_x|user_b".into(),
                text: "second".into(),
            })
            .await
            .unwrap();
        let calls = rec.send_calls.lock();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], "t|1000002|user_a|first");
        assert_eq!(calls[1], "t|1000002|user_b|second");
        assert_eq!(*rec.token_calls.lock(), 1, "token cached across calls");
    }
}
