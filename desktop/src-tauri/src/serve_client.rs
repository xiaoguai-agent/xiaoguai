//! HTTP bridge to a local `xiaoguai serve`.
//!
//! The floater issues its HTTP from the Rust side (via `reqwest`), NOT from the
//! webview. Two reasons:
//!   1. CORS — a Tauri webview origin is `tauri://localhost` (macOS) /
//!      `https://tauri.localhost` (Windows), neither of which is a loopback
//!      origin, so the serve's CORS predicate would not reflect them and a
//!      webview `fetch` to `:7600` would be blocked. Going through Rust skips
//!      the browser CORS check entirely.
//!   2. SSE — streaming `text/event-stream` and re-emitting frames to the UI is
//!      cleaner as a Tauri event channel than a webview `ReadableStream`.
//!
//! Wire contract (mirrors `crates/xiaoguai-api/src/routes/sessions.rs` +
//! `src/sse.rs`):
//!   * `POST /v1/sessions` body `{ user_id, model }` -> `{ id, ... }`.
//!     `model: ""` lets the server pick its default provider/model.
//!   * `POST /v1/sessions/{id}/messages` body `{ content, model?, mode? }` ->
//!     an SSE stream. Each frame is `event: <tag>` / `data: <json>` /
//!     `id: <seq>` lines terminated by a blank line, where `<tag>` is the
//!     `AgentEvent` variant (`text_delta`, `done`, `error`, ...) and `<json>`
//!     is that variant's serde body (it already carries a `type` field equal
//!     to `<tag>`). We forward the parsed `data` JSON object to the frontend
//!     verbatim.

use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter as _};

use crate::config::{AppConfig, FLOATER_USER_ID};

/// Tauri event channel the Rust side emits chat frames on. The frontend
/// listens with `listen("chat://event", ...)`.
pub const CHAT_EVENT: &str = "chat://event";

/// Errors surfaced to the frontend as a structured failure (never panics the
/// command). The `Display` text is what the UI shows.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error("cannot reach xiaoguai serve at {url} — is it running? ({source})")]
    Connect {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("not authorized (401). Set XIAOGUAI_FLOATER_USER/PASS or _TOKEN.")]
    Unauthorized,
    #[error("server returned HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("unexpected response from serve: {0}")]
    Decode(String),
}

// Tauri commands must return a serialisable error; map to a tagged string.
impl Serialize for ServeError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

/// Subset of `SessionResponse` we need — just the id. Extra fields on the wire
/// are ignored by serde.
#[derive(Debug, Deserialize)]
struct SessionResponse {
    id: String,
}

#[derive(Debug, Serialize)]
struct CreateSessionRequest<'a> {
    user_id: &'a str,
    /// Empty string => the server substitutes its default model at chat time.
    model: &'a str,
}

#[derive(Debug, Serialize)]
struct SendMessageRequest<'a> {
    content: &'a str,
    /// Omitted => server default model. The floater always lets the server
    /// choose, so this stays `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<&'a str>,
}

/// One frame forwarded to the frontend over [`CHAT_EVENT`]. `data` is the raw
/// `AgentEvent` JSON object as emitted by the server (it already contains a
/// `type` field). We additionally emit a `stream_end` lifecycle frame the
/// server doesn't, so the UI can always settle its "thinking" state.
///
/// Transport errors are NOT frames — they surface as the command's `Err`
/// (the `invoke('send_message')` promise rejects), which the frontend catches
/// directly.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatFrame {
    /// A server-sent `AgentEvent`. `data.type` is the variant tag.
    Agent { data: serde_json::Value },
    /// Clean end of the SSE stream (the reader hit EOF). The UI re-enables the
    /// composer here even if no terminal `done` frame arrived.
    StreamEnd,
}

/// Build a `reqwest::Client`. Kept tiny: no proxy, rustls, short connect timeout
/// so "serve not running" fails fast instead of hanging the UI.
fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default()
}

/// Attach the configured `Authorization` header (if any) to a request builder.
fn with_auth(rb: reqwest::RequestBuilder, cfg: &AppConfig) -> reqwest::RequestBuilder {
    match &cfg.auth_header {
        Some(h) => rb.header(reqwest::header::AUTHORIZATION, h),
        None => rb,
    }
}

/// `POST /v1/sessions` — create a fresh session and return its id. `model` is
/// sent empty so the server picks its default provider/model (matches the
/// chat-ui, which sends `model: ''`).
pub async fn create_session(cfg: &AppConfig) -> Result<String, ServeError> {
    let url = format!("{}/v1/sessions", cfg.base_url);
    let client = http_client();
    let body = CreateSessionRequest {
        user_id: FLOATER_USER_ID,
        model: "",
    };
    let resp = with_auth(client.post(&url).json(&body), cfg)
        .send()
        .await
        .map_err(|source| ServeError::Connect {
            url: url.clone(),
            source,
        })?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(ServeError::Unauthorized);
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(ServeError::Http {
            status: status.as_u16(),
            body: truncate(&body, 300),
        });
    }
    let session: SessionResponse = resp
        .json()
        .await
        .map_err(|e| ServeError::Decode(e.to_string()))?;
    Ok(session.id)
}

/// `POST /v1/sessions/{id}/messages` — stream the agent's reply. Each parsed
/// SSE frame is emitted to the frontend over [`CHAT_EVENT`] as a [`ChatFrame`].
/// Resolves once the stream ends (EOF or hard error). The terminal `done` /
/// `error` `AgentEvent` is forwarded like any other frame; this function also
/// emits a final [`ChatFrame::StreamEnd`] so the UI can settle unconditionally.
pub async fn stream_message(
    app: &AppHandle,
    cfg: &AppConfig,
    session_id: &str,
    content: &str,
) -> Result<(), ServeError> {
    let url = format!("{}/v1/sessions/{}/messages", cfg.base_url, session_id);
    let client = http_client();
    let body = SendMessageRequest {
        content,
        model: None,
    };
    let resp = with_auth(
        client
            .post(&url)
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .json(&body),
        cfg,
    )
    .send()
    .await
    .map_err(|source| ServeError::Connect {
        url: url.clone(),
        source,
    })?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(ServeError::Unauthorized);
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(ServeError::Http {
            status: status.as_u16(),
            body: truncate(&body, 300),
        });
    }

    // Drain the byte stream, splitting on the SSE record separator `\n\n`.
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|source| ServeError::Connect {
            url: url.clone(),
            source,
        })?;
        // SSE is UTF-8; lossy decode is safe for forwarding (the JSON inside is
        // always valid UTF-8 from the server).
        buf.push_str(&String::from_utf8_lossy(&bytes));
        drain_sse_records(&mut buf, |data_json| {
            emit_agent_frame(app, data_json);
        });
    }
    // Flush any trailing record that wasn't terminated by `\n\n` before EOF.
    if !buf.trim().is_empty() {
        if let Some(data) = parse_sse_record(&buf) {
            emit_agent_frame(app, data);
        }
    }

    emit_frame(app, ChatFrame::StreamEnd);
    Ok(())
}

/// Emit a parsed agent `data` payload (raw `AgentEvent` JSON) to the frontend.
fn emit_agent_frame(app: &AppHandle, data: serde_json::Value) {
    emit_frame(app, ChatFrame::Agent { data });
}

/// Emit any [`ChatFrame`] to the frontend; a failed emit is non-fatal (the
/// window may be closing).
fn emit_frame(app: &AppHandle, frame: ChatFrame) {
    let _ = app.emit(CHAT_EVENT, frame);
}

/// Pop every complete SSE record (text up to a `\n\n`) from `buf`, calling
/// `on_data` with the parsed `data:` JSON of each one that carries data.
/// Records without a `data:` line (bare keep-alive / `id:`-only frames) are
/// dropped. `buf` is left holding the trailing partial record.
fn drain_sse_records(buf: &mut String, mut on_data: impl FnMut(serde_json::Value)) {
    while let Some(idx) = buf.find("\n\n") {
        let record: String = buf.drain(..idx + 2).collect();
        if let Some(data) = parse_sse_record(&record) {
            on_data(data);
        }
    }
}

/// Parse one SSE record. Concatenates all `data:` lines (per the SSE spec) and
/// JSON-decodes the result. Returns `None` for keep-alive / data-less frames or
/// when the data isn't valid JSON.
fn parse_sse_record(record: &str) -> Option<serde_json::Value> {
    let mut data = String::new();
    for line in record.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            // The server emits a single-line JSON `data:`; a leading space after
            // the colon is optional per spec, so trim_start one.
            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        }
        // `event:` and `id:` lines are ignored — `data` already carries the
        // variant `type`, which is what the UI switches on.
    }
    if data.trim().is_empty() {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(data.trim()).ok()
}

/// Cap a string for inclusion in an error message.
fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_text_delta_record() {
        let rec = "event: text_delta\ndata: {\"type\":\"text_delta\",\"delta\":\"hi\"}\nid: 1\n\n";
        let data = parse_sse_record(rec).expect("should parse");
        assert_eq!(data["type"], "text_delta");
        assert_eq!(data["delta"], "hi");
    }

    #[test]
    fn keepalive_record_without_data_is_dropped() {
        // axum keep-alive comments look like `:\n\n` or `: ping`.
        assert!(parse_sse_record(":\n\n").is_none());
        assert!(parse_sse_record("id: 7\n\n").is_none());
    }

    #[test]
    fn drains_multiple_records_and_keeps_partial_tail() {
        let mut buf = String::from(
            "event: text_delta\ndata: {\"type\":\"text_delta\",\"delta\":\"a\"}\n\n\
             event: text_delta\ndata: {\"type\":\"text_delta\",\"delta\":\"b\"}\n\n\
             event: done\ndata: {\"type\":\"done\"", // partial, no terminator
        );
        let mut seen = Vec::new();
        drain_sse_records(&mut buf, |d| {
            seen.push(d["delta"].as_str().unwrap_or("").to_string())
        });
        assert_eq!(seen, vec!["a", "b"]);
        // The unterminated `done` record stays buffered for the next chunk.
        assert!(buf.contains("done"));
        assert!(!buf.contains("\"a\""));
    }

    #[test]
    fn concatenates_multi_line_data() {
        // Per the SSE spec, every continuation line begins with `data:` at
        // column 0; the parser joins their bodies. (The serve emits single-line
        // data today, but a spec-correct parser must handle the split form.)
        let rec = "event: done\ndata: {\"type\":\ndata: \"done\"}\n\n";
        let data = parse_sse_record(rec).expect("multi-line data should join");
        assert_eq!(data["type"], "done");
    }
}
