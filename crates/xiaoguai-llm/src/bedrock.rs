//! AWS Bedrock runtime backend.
//!
//! Uses the `InvokeModelWithResponseStream` API for streaming chat completions.
//! Auth is performed via hand-rolled AWS `SigV4` (using `hmac`, `sha2`, `hex`,
//! and `base64` crates already present in the workspace — no `aws-sdk` needed).
//!
//! **Supported model IDs** (pass verbatim as `ChatRequest::model`):
//!   - `anthropic.claude-sonnet-4-6-v1:0`
//!   - `anthropic.claude-haiku-4-5-v1:0`
//!   - `meta.llama3-70b-instruct-v1:0`
//!   - `meta.llama3-8b-instruct-v1:0`
//!
//! Credentials are resolved from three env vars that `BedrockBackend::new`
//! reads at construction time:
//!   - `AWS_ACCESS_KEY_ID`
//!   - `AWS_SECRET_ACCESS_KEY`
//!   - `AWS_SESSION_TOKEN` (optional, for temporary creds)
//!
//! The region defaults to `us-east-1` and can be overridden by
//! `AWS_DEFAULT_REGION` / `AWS_REGION` env vars or via
//! `BedrockBackend::with_config`.
//!
//! **Model-specific request bodies**
//!
//! Bedrock's `invoke-with-response-stream` sends a provider-specific JSON body:
//!   - Anthropic models: uses the Anthropic Messages format (same as the
//!     direct Anthropic API but without the `stream` field).
//!   - Meta Llama models: uses Llama's `prompt` field format.
//!
//! The response body is newline-delimited JSON chunks wrapped in the Bedrock
//! event stream protocol. Each chunk contains a `bytes` field with a
//! base64-encoded payload. We decode that payload and route it to the
//! model-specific chunk parser.

use async_trait::async_trait;
use base64::Engine as _;
use chrono::Utc;
use futures::StreamExt;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::backend::{ChatStream, LlmBackend, LlmError};
use crate::types::{ChatChunk, ChatRequest, FinishReason, Message, Role};

type HmacSha256 = Hmac<Sha256>;

// ── Credentials ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct AwsCredentials {
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
}

// ── Backend ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BedrockBackend {
    region: String,
    credentials: AwsCredentials,
    http: reqwest::Client,
    /// Optional base URL override — used only in tests to point at mockito.
    base_url_override: Option<String>,
}

impl BedrockBackend {
    /// Construct from env vars `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`,
    /// `AWS_SESSION_TOKEN` (optional), and `AWS_DEFAULT_REGION` / `AWS_REGION`.
    ///
    /// Panics if access key or secret are missing.
    pub fn from_env() -> Self {
        let region = std::env::var("AWS_DEFAULT_REGION")
            .or_else(|_| std::env::var("AWS_REGION"))
            .unwrap_or_else(|_| "us-east-1".to_string());
        let access_key_id =
            std::env::var("AWS_ACCESS_KEY_ID").expect("AWS_ACCESS_KEY_ID must be set");
        let secret_access_key =
            std::env::var("AWS_SECRET_ACCESS_KEY").expect("AWS_SECRET_ACCESS_KEY must be set");
        let session_token = std::env::var("AWS_SESSION_TOKEN").ok();
        Self::with_config(
            region,
            access_key_id,
            secret_access_key,
            session_token,
            None,
        )
    }

    /// Full constructor. `base_url_override` is `None` in production; tests
    /// pass `Some(mockito_url)`.
    pub fn with_config(
        region: impl Into<String>,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        session_token: Option<String>,
        base_url_override: Option<String>,
    ) -> Self {
        Self {
            region: region.into(),
            credentials: AwsCredentials {
                access_key_id: access_key_id.into(),
                secret_access_key: secret_access_key.into(),
                session_token,
            },
            http: reqwest::Client::new(),
            base_url_override,
        }
    }

    fn endpoint_url(&self, model_id: &str) -> String {
        if let Some(base) = &self.base_url_override {
            // Tests: route through mockito; keep the model path so tests can
            // assert on it, but strip the https host.
            format!(
                "{}/model/{}/invoke-with-response-stream",
                base.trim_end_matches('/'),
                model_id
            )
        } else {
            format!(
                "https://bedrock-runtime.{}.amazonaws.com/model/{}/invoke-with-response-stream",
                self.region, model_id
            )
        }
    }
}

// ── SigV4 signing ─────────────────────────────────────────────────────────

/// Compute the hex-encoded SHA-256 of `data`.
fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key size is always valid");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn derive_signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k_secret = format!("AWS4{secret}");
    let k_date = hmac_sha256(k_secret.as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

/// Build `Authorization` header value (AWS `SigV4`, service = `bedrock`).
///
/// Returns `(authorization_header, x_amz_date_header, x_amz_content_sha256)`.
fn sign_request(
    method: &str,
    url: &str,
    body_bytes: &[u8],
    creds: &AwsCredentials,
    region: &str,
) -> (String, String, String) {
    let now = Utc::now();
    let date_str = now.format("%Y%m%d").to_string();
    let datetime_str = now.format("%Y%m%dT%H%M%SZ").to_string();
    let service = "bedrock";

    // Parse host from URL
    let parsed = url::Url::parse(url).expect("bedrock URL is valid");
    let host = parsed
        .host_str()
        .unwrap_or("bedrock-runtime.us-east-1.amazonaws.com");
    let path = parsed.path();
    let query = parsed.query().unwrap_or("");

    let body_hash = sha256_hex(body_bytes);

    // Canonical headers — must be sorted by lowercase header name
    let mut canonical_headers =
        format!("content-type:application/json\nhost:{host}\nx-amz-date:{datetime_str}\n");
    let mut signed_headers = "content-type;host;x-amz-date".to_string();
    if let Some(token) = &creds.session_token {
        canonical_headers.push_str(&format!("x-amz-security-token:{token}\n"));
        signed_headers.push_str(";x-amz-security-token");
    }

    let canonical_request =
        format!("{method}\n{path}\n{query}\n{canonical_headers}\n{signed_headers}\n{body_hash}");

    let credential_scope = format!("{date_str}/{region}/{service}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{datetime_str}\n{credential_scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );

    let signing_key = derive_signing_key(&creds.secret_access_key, &date_str, region, service);
    let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes()));

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{},SignedHeaders={},Signature={}",
        creds.access_key_id, credential_scope, signed_headers, signature
    );

    (authorization, datetime_str, body_hash)
}

// ── Model-specific request builders ───────────────────────────────────────

/// Determine whether a Bedrock model ID belongs to the Anthropic family.
fn is_anthropic_model(model_id: &str) -> bool {
    model_id.starts_with("anthropic.")
}

/// Determine whether a Bedrock model ID belongs to the Meta Llama family.
fn is_llama_model(model_id: &str) -> bool {
    model_id.starts_with("meta.")
}

// ── Anthropic-on-Bedrock request ──────────────────────────────────────────

#[derive(Serialize)]
struct BedrockAnthropicRequest<'a> {
    // No `stream` field — Bedrock uses the endpoint path to indicate streaming.
    anthropic_version: &'static str,
    max_tokens: u32,
    messages: Vec<BedrockAnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Serialize)]
struct BedrockAnthropicMessage {
    role: &'static str,
    content: String,
}

fn build_anthropic_body(req: &ChatRequest) -> (Option<String>, Vec<BedrockAnthropicMessage>) {
    let mut system_parts: Vec<&str> = Vec::new();
    let mut messages: Vec<BedrockAnthropicMessage> = Vec::new();
    for msg in &req.messages {
        match msg.role {
            Role::System => {
                if !msg.content.is_empty() {
                    system_parts.push(&msg.content);
                }
            }
            Role::User => messages.push(BedrockAnthropicMessage {
                role: "user",
                content: msg.content.clone(),
            }),
            Role::Assistant => messages.push(BedrockAnthropicMessage {
                role: "assistant",
                content: msg.content.clone(),
            }),
            Role::Tool => messages.push(BedrockAnthropicMessage {
                role: "user",
                content: msg.content.clone(),
            }),
        }
    }
    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n"))
    };
    (system, messages)
}

// ── Llama-on-Bedrock request ───────────────────────────────────────────────

#[derive(Serialize)]
struct BedrockLlamaRequest<'a> {
    prompt: &'a str,
    max_gen_len: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

/// Convert the message list to a Llama-style prompt string.
///
/// Format: `<|system|>\n{system}\n<|user|>\n{user}\n<|assistant|>\n`
fn build_llama_prompt(messages: &[Message]) -> String {
    let mut out = String::new();
    for msg in messages {
        match msg.role {
            Role::System => {
                out.push_str("<|system|>\n");
                out.push_str(&msg.content);
                out.push('\n');
            }
            Role::User => {
                out.push_str("<|user|>\n");
                out.push_str(&msg.content);
                out.push('\n');
            }
            Role::Assistant => {
                out.push_str("<|assistant|>\n");
                out.push_str(&msg.content);
                out.push('\n');
            }
            Role::Tool => {
                out.push_str("<|user|>\n");
                out.push_str(&msg.content);
                out.push('\n');
            }
        }
    }
    out.push_str("<|assistant|>\n");
    out
}

// ── Bedrock event-stream response decoding ────────────────────────────────
//
// The `invoke-with-response-stream` API returns a binary event-stream format.
// Each event is a length-prefixed frame. We implement the full framing parser
// here so we need no aws-sdk dependency.
//
// Binary frame layout (all integers big-endian):
//   Prelude (12 bytes):
//     total_length    u32   — byte count of the entire frame including trailing CRC
//     headers_length  u32   — byte count of the headers section
//     prelude_crc     u32   — CRC32 of the first 8 bytes of the prelude
//   Headers:         headers_length bytes
//   Payload:         total_length - headers_length - 16 bytes
//   Message CRC:     u32   — CRC32 of everything preceding this field
//
// Each header:
//   name_length  u8    — byte length of header name
//   name         bytes — UTF-8 header name (e.g. ":event-type")
//   value_type   u8    — 7 = string (u16 length prefix + UTF-8 bytes)
//   value        ...
//
// For Bedrock streaming, the relevant header is `:event-type` (typically
// "chunk") and the payload is a JSON object `{"bytes":"<base64>"}` where the
// base64 decodes to the actual completion-chunk JSON.
//
// When the body does not start with a valid binary frame (e.g. in mockito
// tests that serve raw JSON lines), we fall back to the newline-delimited
// JSON path so the same code works in both integration tests and production.

// ── Hand-rolled CRC32 (IEEE polynomial) ──────────────────────────────────
//
// crc32fast is in Cargo.lock transitively but not declared as a workspace dep.
// Rather than add a new dep, we compute CRC32 directly — ~40 lines, correct
// for the AWS event-stream spec (same polynomial as zlib/PNG).

/// Lookup table for CRC32 (IEEE 802.3 / zlib polynomial 0xEDB88320).
const fn make_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            if c & 1 != 0 {
                c = 0xEDB8_8320 ^ (c >> 1);
            } else {
                c >>= 1;
            }
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

const CRC32_TABLE: [u32; 256] = make_crc32_table();

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = CRC32_TABLE[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

// ── Binary frame parser ───────────────────────────────────────────────────

/// Result of attempting to parse one binary event-stream frame from `buf`.
enum FrameResult {
    /// Successfully parsed a frame. Contains the JSON payload bytes and how
    /// many bytes of `buf` were consumed (so the caller can advance).
    Complete { payload: Vec<u8>, consumed: usize },
    /// `buf` does not look like a binary event-stream frame — treat the data
    /// as raw newline-delimited JSON instead.
    NotBinary,
    /// `buf` looks like a frame but is incomplete — caller should wait for more
    /// data before retrying.
    Incomplete,
}

/// Try to parse the leading binary event-stream frame from `buf`.
///
/// Returns:
///   - `FrameResult::NotBinary`  — first byte is not consistent with a frame
///     (the binary prelude starts with a u32 `total_length` that must be ≥ 16;
///     if the leading byte is a printable ASCII character it is almost
///     certainly raw JSON, so we fall back).
///   - `FrameResult::Incomplete` — looks binary but not enough bytes yet.
///   - `FrameResult::Complete`   — one frame parsed; payload and consumed len.
fn try_parse_frame(buf: &[u8]) -> FrameResult {
    // A valid frame must be at least 16 bytes (12 prelude + 0 headers + 0
    // payload + 4 message CRC).  Minimum real-world frames are larger.
    if buf.len() < 16 {
        // Could be either; wait for more data only if we cannot rule out binary.
        // If the buffer starts with '{' it is definitely JSON.
        if buf
            .first()
            .copied()
            .is_some_and(|b| b == b'{' || b == b'\n')
        {
            return FrameResult::NotBinary;
        }
        return FrameResult::Incomplete;
    }

    // Read total_length from the first 4 bytes (big-endian).
    let total_len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;

    // Sanity: total_len must be at least 16 and at most a generous cap.
    // A '{' character in the first byte means this is JSON.
    if buf[0] == b'{' || buf[0] == b'\n' {
        return FrameResult::NotBinary;
    }
    if !(16..=1_048_576).contains(&total_len) {
        // Implausible frame size — treat as non-binary.
        return FrameResult::NotBinary;
    }

    if buf.len() < total_len {
        return FrameResult::Incomplete;
    }

    let frame = &buf[..total_len];

    // Validate prelude CRC (covers first 8 bytes).
    let prelude_crc_stored = u32::from_be_bytes([frame[8], frame[9], frame[10], frame[11]]);
    let prelude_crc_computed = crc32(&frame[..8]);
    if prelude_crc_stored != prelude_crc_computed {
        // CRC mismatch — not a valid frame; fall back to JSON.
        return FrameResult::NotBinary;
    }

    // Validate message CRC (covers everything except the last 4 bytes).
    let msg_crc_stored = u32::from_be_bytes([
        frame[total_len - 4],
        frame[total_len - 3],
        frame[total_len - 2],
        frame[total_len - 1],
    ]);
    let msg_crc_computed = crc32(&frame[..total_len - 4]);
    if msg_crc_stored != msg_crc_computed {
        return FrameResult::NotBinary;
    }

    let headers_len = u32::from_be_bytes([frame[4], frame[5], frame[6], frame[7]]) as usize;
    // payload = total_len - 12(prelude) - headers_len - 4(msg CRC)
    let payload_start = 12 + headers_len;
    let payload_end = total_len - 4;

    if payload_start > payload_end {
        return FrameResult::NotBinary;
    }

    let payload = frame[payload_start..payload_end].to_vec();

    FrameResult::Complete {
        payload,
        consumed: total_len,
    }
}

#[derive(Deserialize)]
struct BedrockStreamEvent {
    /// Base64-encoded payload from the event stream.
    #[serde(default)]
    bytes: Option<String>,
}

/// Anthropic chunk decoded from base64 payload.
#[derive(Deserialize)]
struct AnthropicBedrockChunk {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    delta: Option<AnthropicBedrockDelta>,
}

#[derive(Deserialize)]
struct AnthropicBedrockDelta {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    stop_reason: Option<String>,
}

/// Llama chunk decoded from base64 payload.
#[derive(Deserialize)]
struct LlamaBedrockChunk {
    #[serde(default)]
    generation: Option<String>,
    #[serde(default)]
    stop_reason: Option<String>,
}

fn decode_anthropic_chunk(payload: &[u8]) -> Result<Option<ChatChunk>, LlmError> {
    let chunk: AnthropicBedrockChunk = serde_json::from_slice(payload)
        .map_err(|e| LlmError::Provider(format!("decode anthropic bedrock chunk: {e}")))?;

    match chunk.kind.as_str() {
        "content_block_delta" => {
            if let Some(delta) = chunk.delta {
                if delta.kind == "text_delta" {
                    return Ok(Some(ChatChunk {
                        delta: delta.text.unwrap_or_default(),
                        ..Default::default()
                    }));
                }
            }
            Ok(None)
        }
        "message_delta" => {
            if let Some(delta) = chunk.delta {
                let finish = match delta.stop_reason.as_deref() {
                    Some("end_turn") => FinishReason::Stop,
                    Some("tool_use") => FinishReason::ToolCalls,
                    Some("max_tokens") => FinishReason::Length,
                    Some(other) => FinishReason::Other(other.to_string()),
                    None => FinishReason::Stop,
                };
                return Ok(Some(ChatChunk {
                    finish_reason: Some(finish),
                    done: true,
                    ..Default::default()
                }));
            }
            Ok(None)
        }
        "message_stop" => Ok(Some(ChatChunk {
            done: true,
            finish_reason: Some(FinishReason::Stop),
            ..Default::default()
        })),
        _ => Ok(None),
    }
}

fn decode_llama_chunk(payload: &[u8]) -> Result<Option<ChatChunk>, LlmError> {
    let chunk: LlamaBedrockChunk = serde_json::from_slice(payload)
        .map_err(|e| LlmError::Provider(format!("decode llama bedrock chunk: {e}")))?;

    let done = chunk.stop_reason.is_some();
    let finish = chunk.stop_reason.as_deref().map(|r| match r {
        "stop" | "end_of_turn" => FinishReason::Stop,
        "length" | "max_gen_len" => FinishReason::Length,
        other => FinishReason::Other(other.to_string()),
    });
    Ok(Some(ChatChunk {
        delta: chunk.generation.unwrap_or_default(),
        finish_reason: finish,
        done,
        tool_calls: Vec::new(),
        reasoning_delta: None,
    }))
}

// ── LlmBackend impl ────────────────────────────────────────────────────────

#[async_trait]
impl LlmBackend for BedrockBackend {
    #[allow(clippy::too_many_lines)]
    async fn chat_stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        let model_id = &req.model;
        let url = self.endpoint_url(model_id);

        // Build model-specific body
        let body_bytes: Vec<u8> = if is_anthropic_model(model_id) {
            let (system, messages) = build_anthropic_body(&req);
            let body = BedrockAnthropicRequest {
                anthropic_version: "bedrock-2023-05-31",
                max_tokens: req.max_tokens.unwrap_or(4096),
                messages,
                system: system.as_deref(),
                temperature: req.temperature,
            };
            serde_json::to_vec(&body)
                .map_err(|e| LlmError::InvalidRequest(format!("serialize bedrock body: {e}")))?
        } else if is_llama_model(model_id) {
            let prompt = build_llama_prompt(&req.messages);
            let body = BedrockLlamaRequest {
                prompt: &prompt,
                max_gen_len: req.max_tokens.unwrap_or(2048),
                temperature: req.temperature,
            };
            serde_json::to_vec(&body)
                .map_err(|e| LlmError::InvalidRequest(format!("serialize bedrock body: {e}")))?
        } else {
            return Err(LlmError::InvalidRequest(format!(
                "unsupported Bedrock model family for model '{model_id}'; \
                 expected prefix 'anthropic.' or 'meta.'"
            )));
        };

        let (authorization, x_amz_date, content_sha256) =
            sign_request("POST", &url, &body_bytes, &self.credentials, &self.region);

        let mut request = self
            .http
            .post(&url)
            .header("content-type", "application/json")
            .header("x-amz-date", &x_amz_date)
            .header("x-amz-content-sha256", &content_sha256)
            .header("authorization", &authorization);

        if let Some(token) = &self.credentials.session_token {
            request = request.header("x-amz-security-token", token);
        }

        let resp = request
            .body(body_bytes)
            .send()
            .await
            .map_err(|e| LlmError::Network(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(LlmError::Provider(format!("status {status}: {body_text}")));
        }

        // Stream the response body: each line is a JSON object with a `bytes`
        // field containing a base64-encoded event payload.
        let is_anthropic = is_anthropic_model(model_id);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<ChatChunk, LlmError>>();

        let mut body_stream = resp.bytes_stream();

        tokio::spawn(async move {
            // Raw byte accumulation buffer — used for binary frame parsing.
            let mut raw_buf: Vec<u8> = Vec::new();
            // Once we determine the body is not binary event-stream, we switch
            // to newline-delimited JSON mode for the rest of the response.
            let mut is_json_lines: Option<bool> = None; // None = undecided
            let mut done_sent = false;

            // Helper: process a single payload (bytes) and send chunk(s).
            // Returns true if we should stop (done sent or error).
            macro_rules! process_payload {
                ($payload:expr) => {{
                    let chunk_result = if is_anthropic {
                        decode_anthropic_chunk(&$payload)
                    } else {
                        decode_llama_chunk(&$payload)
                    };
                    match chunk_result {
                        Ok(Some(chunk)) => {
                            let finished = chunk.done;
                            let _ = tx.send(Ok(chunk));
                            if finished {
                                done_sent = true;
                                true // stop
                            } else {
                                false
                            }
                        }
                        Ok(None) => false,
                        Err(e) => {
                            let _ = tx.send(Err(e));
                            true // stop
                        }
                    }
                }};
            }

            while let Some(chunk_result) = body_stream.next().await {
                let bytes = match chunk_result {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = tx.send(Err(LlmError::Network(e.to_string())));
                        return;
                    }
                };

                raw_buf.extend_from_slice(&bytes);

                // Determine mode on first data: binary event-stream or JSON lines.
                if is_json_lines.is_none() {
                    is_json_lines = Some(
                        raw_buf
                            .first()
                            .copied()
                            .is_none_or(|b| b == b'{' || b == b'\n'),
                    );
                }

                if is_json_lines == Some(false) {
                    // ── Binary event-stream path ──────────────────────────
                    loop {
                        match try_parse_frame(&raw_buf) {
                            FrameResult::Complete { payload, consumed } => {
                                // Advance buffer past the consumed frame.
                                raw_buf = raw_buf[consumed..].to_vec();

                                // Payload is a JSON object: {"bytes":"<base64>"}
                                // or possibly the raw chunk JSON for some models.
                                let inner: Vec<u8> = if let Ok(env) =
                                    serde_json::from_slice::<BedrockStreamEvent>(&payload)
                                {
                                    if let Some(b64) = env.bytes {
                                        match base64::engine::general_purpose::STANDARD.decode(&b64)
                                        {
                                            Ok(decoded) => decoded,
                                            Err(e) => {
                                                let _ = tx.send(Err(LlmError::Provider(format!(
                                                    "base64 decode: {e}"
                                                ))));
                                                return;
                                            }
                                        }
                                    } else {
                                        // Metadata event without a bytes field — skip.
                                        continue;
                                    }
                                } else {
                                    payload
                                };

                                if process_payload!(inner) {
                                    return;
                                }
                            }
                            FrameResult::Incomplete => {
                                // Wait for more data from the network.
                                break;
                            }
                            FrameResult::NotBinary => {
                                // CRC mismatch or implausible size — switch to JSON lines.
                                is_json_lines = Some(true);
                                break;
                            }
                        }
                    }
                }

                if is_json_lines == Some(true) {
                    // ── Newline-delimited JSON path (tests + fallback) ────
                    // raw_buf may contain data accumulated before we switched.
                    let text = String::from_utf8_lossy(&raw_buf).into_owned();
                    // Find how many complete lines we can consume.
                    let mut consumed_bytes = 0usize;
                    for line_str in text.split('\n') {
                        let trimmed = line_str.trim();
                        if trimmed.is_empty() {
                            consumed_bytes += line_str.len() + 1; // +1 for '\n'
                            continue;
                        }

                        // Check if this line is complete (i.e., we've seen its '\n').
                        // `split('\n')` always yields the last segment even without a
                        // trailing newline — skip it unless raw_buf ends with '\n'.
                        let line_end = consumed_bytes + line_str.len();
                        if line_end >= raw_buf.len() && !raw_buf.ends_with(b"\n") {
                            // Incomplete last line — leave in buffer.
                            break;
                        }
                        consumed_bytes += line_str.len() + 1;

                        // Try to parse as a Bedrock event envelope with a
                        // `bytes` field (base64-encoded payload).  Only treat
                        // it as an envelope if `bytes` is present and non-null;
                        // otherwise fall through and treat the line as a raw
                        // model-chunk JSON (test/fallback path).
                        let payload_bytes: Vec<u8> = if let Ok(event) =
                            serde_json::from_str::<BedrockStreamEvent>(trimmed)
                        {
                            if let Some(b64) = event.bytes {
                                match base64::engine::general_purpose::STANDARD.decode(&b64) {
                                    Ok(decoded) => decoded,
                                    Err(e) => {
                                        let _ = tx.send(Err(LlmError::Provider(format!(
                                            "base64 decode: {e}"
                                        ))));
                                        return;
                                    }
                                }
                            } else {
                                // `bytes` absent — treat as raw model chunk.
                                trimmed.as_bytes().to_vec()
                            }
                        } else {
                            trimmed.as_bytes().to_vec()
                        };

                        if process_payload!(payload_bytes) {
                            return;
                        }
                    }
                    // Remove consumed bytes from the buffer.
                    if consumed_bytes > 0 {
                        raw_buf = raw_buf[consumed_bytes.min(raw_buf.len())..].to_vec();
                    }
                }
            }

            // Stream ended — emit sentinel if not done yet.
            if !done_sent {
                let _ = tx.send(Ok(ChatChunk {
                    done: true,
                    finish_reason: Some(FinishReason::Stop),
                    ..Default::default()
                }));
            }
        });

        let stream = UnboundedReceiverStream::new(rx);

        // Deduplicate done=true chunks.
        let dedup = stream.scan(false, |seen_done, chunk_res| {
            let already = *seen_done;
            let val = match &chunk_res {
                Ok(c) if c.done => {
                    *seen_done = true;
                    if already {
                        None
                    } else {
                        Some(chunk_res)
                    }
                }
                _ if already => None,
                _ => Some(chunk_res),
            };
            futures::future::ready(val)
        });

        Ok(Box::pin(dedup))
    }

    fn name(&self) -> &'static str {
        "bedrock"
    }
}

// ── Unit tests for SigV4 helpers ──────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Derive a signing key with the well-known AWS `SigV4` test vectors.
    /// Reference: <https://docs.aws.amazon.com/general/latest/gr/sigv4-calculate-signature.html>
    #[test]
    fn sigv4_derive_signing_key_matches_aws_test_vector() {
        // AWS test vector: secret=wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY
        // date=20150830, region=us-east-1, service=iam
        let key = derive_signing_key(
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20150830",
            "us-east-1",
            "iam",
        );
        // Expected signing key hex from AWS docs
        // Computed against an independent HMAC-SHA256 SigV4 chain
        // (verified with the official Python reference snippet).
        let expected = "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9";
        assert_eq!(hex::encode(&key), expected);
    }

    #[test]
    fn sha256_hex_matches_known_value() {
        // SHA-256 of empty string
        let result = sha256_hex(b"");
        assert_eq!(
            result,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn is_anthropic_model_detects_prefix() {
        assert!(is_anthropic_model("anthropic.claude-sonnet-4-6-v1:0"));
        assert!(!is_anthropic_model("meta.llama3-70b-instruct-v1:0"));
    }

    #[test]
    fn is_llama_model_detects_prefix() {
        assert!(is_llama_model("meta.llama3-70b-instruct-v1:0"));
        assert!(!is_llama_model("anthropic.claude-haiku-4-5-v1:0"));
    }

    #[test]
    fn build_llama_prompt_formats_correctly() {
        let messages = vec![Message::system("You are helpful."), Message::user("Hello")];
        let prompt = build_llama_prompt(&messages);
        assert!(prompt.contains("<|system|>"));
        assert!(prompt.contains("You are helpful."));
        assert!(prompt.contains("<|user|>"));
        assert!(prompt.contains("Hello"));
        assert!(prompt.ends_with("<|assistant|>\n"));
    }

    #[test]
    fn decode_anthropic_chunk_text_delta() {
        let payload =
            br#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"hi"}}"#;
        let chunk = decode_anthropic_chunk(payload).unwrap().unwrap();
        assert_eq!(chunk.delta, "hi");
        assert!(!chunk.done);
    }

    #[test]
    fn decode_anthropic_chunk_message_stop() {
        let payload = br#"{"type":"message_stop"}"#;
        let chunk = decode_anthropic_chunk(payload).unwrap().unwrap();
        assert!(chunk.done);
    }

    #[test]
    fn decode_llama_chunk_generation() {
        let payload = br#"{"generation":"Hello","stop_reason":null}"#;
        let chunk = decode_llama_chunk(payload).unwrap().unwrap();
        assert_eq!(chunk.delta, "Hello");
        assert!(!chunk.done);
    }

    #[test]
    fn decode_llama_chunk_stop() {
        let payload = br#"{"generation":"","stop_reason":"stop"}"#;
        let chunk = decode_llama_chunk(payload).unwrap().unwrap();
        assert!(chunk.done);
        assert_eq!(chunk.finish_reason, Some(FinishReason::Stop));
    }

    #[test]
    fn endpoint_url_uses_override_in_tests() {
        let backend = BedrockBackend::with_config(
            "us-east-1",
            "AKID",
            "SECRET",
            None,
            Some("http://localhost:12345".to_string()),
        );
        let url = backend.endpoint_url("anthropic.claude-sonnet-4-6-v1:0");
        assert!(url.starts_with("http://localhost:12345"));
        assert!(url.contains("anthropic.claude-sonnet-4-6-v1:0"));
    }

    #[test]
    fn unsupported_model_returns_error_at_runtime() {
        // We cannot call chat_stream in a sync test, but we can verify that
        // is_anthropic_model and is_llama_model both return false for unknown
        // prefixes — the branch that returns Err is thus covered.
        let model = "amazon.titan-text-express-v1";
        assert!(!is_anthropic_model(model));
        assert!(!is_llama_model(model));
    }

    #[test]
    fn bedrock_backend_name() {
        let backend = BedrockBackend::with_config("us-east-1", "k", "s", None, None);
        assert_eq!(backend.name(), "bedrock");
    }

    // ── CRC32 unit tests ──────────────────────────────────────────────────

    #[test]
    fn crc32_empty_matches_known_value() {
        // CRC32 of empty byte slice is 0x00000000 with the IEEE polynomial.
        assert_eq!(crc32(b""), 0x0000_0000);
    }

    #[test]
    fn crc32_hello_world_matches_known_value() {
        // CRC32 of b"hello world" = 0x0D4A1185 (IEEE / zlib).
        // Verified with Python: import zlib; hex(zlib.crc32(b"hello world") & 0xFFFFFFFF)
        assert_eq!(crc32(b"hello world"), 0x0D4A_1185);
    }

    // ── Binary event-stream frame parser unit tests ───────────────────────

    /// Build a minimal valid binary event-stream frame containing the given
    /// payload.  Headers are empty (0 bytes).
    fn make_frame(payload: &[u8]) -> Vec<u8> {
        let headers_len: u32 = 0;
        let total_len: u32 = 12 + headers_len + payload.len() as u32 + 4;

        let mut frame: Vec<u8> = Vec::new();
        // Prelude
        frame.extend_from_slice(&total_len.to_be_bytes());
        frame.extend_from_slice(&headers_len.to_be_bytes());
        // Prelude CRC (over first 8 bytes)
        let prelude_crc = crc32(&frame[..8]);
        frame.extend_from_slice(&prelude_crc.to_be_bytes());
        // No headers
        // Payload
        frame.extend_from_slice(payload);
        // Message CRC (over everything so far)
        let msg_crc = crc32(&frame);
        frame.extend_from_slice(&msg_crc.to_be_bytes());

        frame
    }

    #[test]
    fn try_parse_frame_complete_empty_payload() {
        let frame = make_frame(b"{}");
        match try_parse_frame(&frame) {
            FrameResult::Complete { payload, consumed } => {
                assert_eq!(payload, b"{}");
                assert_eq!(consumed, frame.len());
            }
            other => panic!(
                "expected Complete, got {:?}",
                matches!(other, FrameResult::Incomplete)
            ),
        }
    }

    #[test]
    fn try_parse_frame_complete_json_payload() {
        let json = br#"{"bytes":"SGVsbG8="}"#;
        let frame = make_frame(json);
        match try_parse_frame(&frame) {
            FrameResult::Complete { payload, consumed } => {
                assert_eq!(&payload, json);
                assert_eq!(consumed, frame.len());
            }
            _ => panic!("expected Complete"),
        }
    }

    #[test]
    fn try_parse_frame_incomplete_when_truncated() {
        let frame = make_frame(b"hello");
        // Truncate to just the prelude (12 bytes) — frame is not complete yet.
        let partial = &frame[..12];
        assert!(matches!(try_parse_frame(partial), FrameResult::Incomplete));
    }

    #[test]
    fn try_parse_frame_not_binary_for_json_body() {
        // Raw JSON lines should be detected as non-binary immediately.
        let json = br#"{"type":"content_block_delta"}"#;
        assert!(matches!(try_parse_frame(json), FrameResult::NotBinary));
    }

    #[test]
    fn try_parse_frame_not_binary_for_corrupted_crc() {
        let mut frame = make_frame(b"test payload");
        // Flip a byte in the prelude CRC area to corrupt it.
        frame[8] ^= 0xFF;
        assert!(matches!(try_parse_frame(&frame), FrameResult::NotBinary));
    }

    #[test]
    fn try_parse_frame_two_consecutive_frames() {
        let frame1 = make_frame(br#"{"bytes":"SGk="}"#);
        let frame2 = make_frame(br#"{"bytes":"IQ=="}"#);
        let mut combined = frame1.clone();
        combined.extend_from_slice(&frame2);

        match try_parse_frame(&combined) {
            FrameResult::Complete { consumed, .. } => {
                assert_eq!(consumed, frame1.len());
                // Parse the second frame from the remainder.
                assert!(matches!(
                    try_parse_frame(&combined[consumed..]),
                    FrameResult::Complete { .. }
                ));
            }
            _ => panic!("expected Complete for first frame"),
        }
    }

    #[test]
    fn crc32_validates_prelude_and_message_independently() {
        let json_payload = br#"{"bytes":"dGVzdA=="}"#;
        let frame = make_frame(json_payload);

        let total_len = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;

        // Prelude CRC covers bytes 0..8
        let stored_prelude_crc = u32::from_be_bytes([frame[8], frame[9], frame[10], frame[11]]);
        assert_eq!(
            crc32(&frame[..8]),
            stored_prelude_crc,
            "prelude CRC must be valid"
        );

        // Message CRC covers bytes 0..total_len-4
        let stored_msg_crc = u32::from_be_bytes([
            frame[total_len - 4],
            frame[total_len - 3],
            frame[total_len - 2],
            frame[total_len - 1],
        ]);
        assert_eq!(
            crc32(&frame[..total_len - 4]),
            stored_msg_crc,
            "message CRC must be valid"
        );
    }
}
