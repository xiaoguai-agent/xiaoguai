//! `xiaoguai remote ...` — drive a running `xiaoguai-api` over HTTP.
//!
//! Reuses the same wire shapes the server publishes (`SessionResponse`,
//! `CreateSessionRequest`, `SendMessageRequest`) by depending on
//! serde-compatible JSON; we don't pull `xiaoguai-api` into the CLI's
//! prod dependency graph to keep the binary small.

use anyhow::{anyhow, bail, Context, Result};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Clone)]
pub struct RemoteClient {
    base_url: String,
    http: reqwest::Client,
}

/// Build the HTTP client used for every remote call. When both
/// `XIAOGUAI_AUTH__USERNAME` and `XIAOGUAI_AUTH__PASSWORD` are set (the same
/// env vars the server reads for its SEC-01 Basic-auth gate), attach an owner
/// `Authorization: Basic …` default header so `xiaoguai cli`/`chat`/`remote`
/// can reach a server that has auth enabled. Unset ⇒ an unauthenticated client
/// (loopback / no-auth deployments are unaffected). A `--server` URL that
/// embeds `user:pass@host` also works — reqwest applies that per-request.
fn build_http_client() -> reqwest::Client {
    use base64::Engine as _;
    let user = std::env::var("XIAOGUAI_AUTH__USERNAME").unwrap_or_default();
    let pass = std::env::var("XIAOGUAI_AUTH__PASSWORD").unwrap_or_default();
    if user.is_empty() || pass.is_empty() {
        return reqwest::Client::new();
    }
    let token = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
    let mut value = match reqwest::header::HeaderValue::from_str(&format!("Basic {token}")) {
        Ok(v) => v,
        Err(_) => return reqwest::Client::new(),
    };
    value.set_sensitive(true);
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::AUTHORIZATION, value);
    reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

impl RemoteClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http: build_http_client(),
        }
    }

    /// Check the server health endpoint.
    ///
    /// # Errors
    /// Returns an error if the HTTP request fails or the server returns a
    /// non-2xx status.
    pub async fn healthz(&self) -> Result<String> {
        let resp = self
            .http
            .get(format!("{}/healthz", self.base_url))
            .send()
            .await
            .context("GET /healthz")?;
        if !resp.status().is_success() {
            bail!("healthz status {}", resp.status());
        }
        Ok(resp.text().await.unwrap_or_default())
    }

    /// Create a new chat session on the remote server.
    ///
    /// # Errors
    /// Returns an error if the HTTP request fails, the server returns a
    /// non-2xx status, or the response body cannot be decoded.
    pub async fn create_session(&self, req: &CreateSessionRequest) -> Result<SessionResponse> {
        let resp = self
            .http
            .post(format!("{}/v1/sessions", self.base_url))
            .json(req)
            .send()
            .await
            .context("POST /v1/sessions")?;
        require_2xx(&resp)?;
        let parsed: SessionResponse = resp.json().await.context("decode session body")?;
        Ok(parsed)
    }

    /// Retrieve the message history for a session.
    ///
    /// # Errors
    /// Returns an error if the HTTP request fails, the server returns a
    /// non-2xx status, or the response body cannot be decoded.
    pub async fn list_messages(&self, session_id: &str) -> Result<Vec<JsonValue>> {
        let resp = self
            .http
            .get(format!(
                "{}/v1/sessions/{session_id}/messages",
                self.base_url
            ))
            .send()
            .await
            .context("GET messages")?;
        require_2xx(&resp)?;
        let v: Vec<JsonValue> = resp.json().await.context("decode messages body")?;
        Ok(v)
    }

    /// Cancel an in-flight session. Returns `true` if the server confirmed
    /// cancellation.
    ///
    /// # Errors
    /// Returns an error if the HTTP request fails, the server returns a
    /// non-2xx status, or the response body cannot be decoded.
    pub async fn cancel(&self, session_id: &str) -> Result<bool> {
        let resp = self
            .http
            .post(format!("{}/v1/sessions/{session_id}/cancel", self.base_url))
            .json(&serde_json::json!({}))
            .send()
            .await
            .context("POST cancel")?;
        require_2xx(&resp)?;
        let body: serde_json::Value = resp.json().await?;
        Ok(body
            .get("cancelled")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false))
    }

    /// `POST /v1/loops` — create + arm a session-scoped recurring loop.
    ///
    /// # Errors
    /// Returns a teaching error when the server is unreachable or returns a
    /// non-2xx status (the body carries the reason — 404 unknown session,
    /// 409 archived / already-has-a-loop, 503 loops unwired).
    pub async fn create_loop(&self, req: &CreateLoopRequest) -> Result<LoopResponse> {
        let resp = self
            .http
            .post(format!("{}/v1/loops", self.base_url))
            .json(req)
            .send()
            .await
            .context("POST /v1/loops")?;
        require_2xx_with_body(resp).await
    }

    /// `GET /v1/loops` — list all loops, newest first.
    ///
    /// # Errors
    /// Returns an error if the request fails or the body cannot be decoded.
    pub async fn list_loops(&self) -> Result<Vec<LoopResponse>> {
        let resp = self
            .http
            .get(format!("{}/v1/loops", self.base_url))
            .send()
            .await
            .context("GET /v1/loops")?;
        require_2xx(&resp)?;
        resp.json().await.context("decode loops body")
    }

    /// `GET /v1/loops/:id`.
    ///
    /// # Errors
    /// Returns an error if the request fails, the loop is unknown, or the
    /// body cannot be decoded.
    pub async fn get_loop(&self, id: &str) -> Result<LoopResponse> {
        let resp = self
            .http
            .get(format!("{}/v1/loops/{id}", self.base_url))
            .send()
            .await
            .context("GET /v1/loops/:id")?;
        require_2xx_with_body(resp).await
    }

    /// `DELETE /v1/loops/:id` — cancel a live loop.
    ///
    /// # Errors
    /// Returns a teaching error carrying the server's reason (404 unknown,
    /// 409 already terminal).
    pub async fn cancel_loop(&self, id: &str) -> Result<LoopResponse> {
        let resp = self
            .http
            .delete(format!("{}/v1/loops/{id}", self.base_url))
            .send()
            .await
            .context("DELETE /v1/loops/:id")?;
        require_2xx_with_body(resp).await
    }

    /// `POST /v1/loops/:id/resume` — resume a paused loop.
    ///
    /// # Errors
    /// Returns a teaching error carrying the server's reason (404 unknown,
    /// 409 not paused).
    pub async fn resume_loop(&self, id: &str) -> Result<LoopResponse> {
        let resp = self
            .http
            .post(format!("{}/v1/loops/{id}/resume", self.base_url))
            .send()
            .await
            .context("POST /v1/loops/:id/resume")?;
        require_2xx_with_body(resp).await
    }

    /// `POST /v1/sessions/:id/messages` using the session's own model. Thin
    /// wrapper over [`Self::send_message_with_model`] with no override.
    ///
    /// # Errors
    /// See [`Self::send_message_with_model`].
    pub async fn send_message<F>(
        &self,
        session_id: &str,
        content: &str,
        on_event: F,
    ) -> Result<()>
    where
        F: FnMut(RemoteEvent) -> Result<()>,
    {
        self.send_message_with_model(session_id, content, None, on_event)
            .await
    }

    /// `POST /v1/sessions/:id/messages` — drain the SSE stream into the
    /// provided sink. `model` overrides the session's model for this one
    /// message (`None` or empty → the session default); the server honours it
    /// via `SendMessageRequest.model`. The sink receives one `RemoteEvent` per
    /// line and may stop the stream by returning `Err`.
    ///
    /// # Errors
    /// Returns an error if the HTTP request fails, the server returns a
    /// non-2xx status, an SSE frame cannot be decoded, or `on_event` returns
    /// an error.
    pub async fn send_message_with_model<F>(
        &self,
        session_id: &str,
        content: &str,
        model: Option<&str>,
        on_event: F,
    ) -> Result<()>
    where
        F: FnMut(RemoteEvent) -> Result<()>,
    {
        let mut body = serde_json::Map::new();
        body.insert("content".into(), JsonValue::String(content.to_string()));
        if let Some(m) = model.filter(|m| !m.is_empty()) {
            body.insert("model".into(), JsonValue::String(m.to_string()));
        }
        let resp = self
            .http
            .post(format!(
                "{}/v1/sessions/{session_id}/messages",
                self.base_url
            ))
            .json(&JsonValue::Object(body))
            .send()
            .await
            .context("POST messages")?;
        require_2xx(&resp)?;
        drain_sse(resp, &["done", "error"], on_event).await
    }

    /// `POST /v1/sessions/:id/orchestrate` — run `goal` through an expert team
    /// (members run in parallel, the lead synthesizes one answer) and drain the
    /// `OrchestrateEvent` SSE stream into `on_event`, stopping after the
    /// terminal `final` event.
    ///
    /// # Errors
    /// Returns a teaching error when the request fails or the server returns a
    /// non-2xx status (404 unknown session, 409 a turn is already in flight,
    /// 422 no team matches the goal, 503 orchestration unwired — the body
    /// carries the reason), or an SSE frame can't be decoded.
    pub async fn orchestrate<F>(
        &self,
        session_id: &str,
        req: &OrchestrateRequest,
        on_event: F,
    ) -> Result<()>
    where
        F: FnMut(RemoteEvent) -> Result<()>,
    {
        let resp = self
            .http
            .post(format!(
                "{}/v1/sessions/{session_id}/orchestrate",
                self.base_url
            ))
            .json(req)
            .send()
            .await
            .context("POST orchestrate")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            let detail = serde_json::from_str::<JsonValue>(&body)
                .ok()
                .and_then(|v| {
                    v.get("message")
                        .or_else(|| v.get("error"))
                        .and_then(JsonValue::as_str)
                        .map(ToOwned::to_owned)
                })
                .unwrap_or(body);
            bail!("orchestrate returned {status}: {detail}");
        }
        drain_sse(resp, &["final", "error"], on_event).await
    }
}

/// Drain an SSE response into `on_event`, stopping after an event whose name is
/// in `terminal` (or at stream end). Shared by `send_message_with_model`
/// (terminal `done`/`error`) and `orchestrate` (terminal `final`/`error`).
async fn drain_sse<F>(resp: reqwest::Response, terminal: &[&str], mut on_event: F) -> Result<()>
where
    F: FnMut(RemoteEvent) -> Result<()>,
{
    let mut stream = resp.bytes_stream().eventsource();
    while let Some(ev) = stream.next().await {
        let ev = ev.context("sse event")?;
        let data: JsonValue = serde_json::from_str(&ev.data)
            .with_context(|| format!("decode sse data: {}", ev.data))?;
        let remote = RemoteEvent {
            name: ev.event,
            payload: data,
        };
        let stop = terminal.contains(&remote.name.as_str());
        on_event(remote)?;
        if stop {
            break;
        }
    }
    Ok(())
}

fn require_2xx(resp: &reqwest::Response) -> Result<()> {
    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else {
        Err(anyhow!("remote returned status {status}"))
    }
}

/// Decode a 2xx JSON body, or surface the server's error envelope on a
/// non-2xx so the operator sees the teaching message (the loop routes
/// return `{code, message}` with actionable detail).
async fn require_2xx_with_body<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
) -> Result<T> {
    let status = resp.status();
    if status.is_success() {
        return resp.json().await.context("decode response body");
    }
    let body = resp.text().await.unwrap_or_default();
    let detail = serde_json::from_str::<JsonValue>(&body)
        .ok()
        .and_then(|v| {
            v.get("message")
                .or_else(|| v.get("error"))
                .and_then(JsonValue::as_str)
                .map(ToOwned::to_owned)
        })
        .unwrap_or(body);
    Err(anyhow!("remote returned {status}: {detail}"))
}

#[derive(Debug, Serialize)]
pub struct CreateLoopRequest {
    pub session_id: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval_secs: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_ticks: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_secs: Option<u32>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dynamic_pacing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_interval_secs: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_interval_secs: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_total_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct LoopResponse {
    pub id: String,
    pub session_id: String,
    pub prompt: String,
    #[serde(default = "default_pacing")]
    pub pacing_kind: String,
    pub interval_secs: u32,
    #[serde(default)]
    pub max_total_tokens: u64,
    pub max_ticks: u32,
    pub ttl_secs: u32,
    pub status: String,
    pub next_tick_at: String,
    pub ticks_run: u32,
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default)]
    pub last_error: Option<String>,
}

fn default_pacing() -> String {
    "fixed".to_string()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub user_id: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Body for `POST /v1/sessions/:id/orchestrate`. `team_id` omitted auto-routes
/// the goal to the best-matching active team; `max_members` caps fan-out (1–8).
#[derive(Debug, Serialize)]
pub struct OrchestrateRequest {
    pub goal: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_members: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionResponse {
    pub id: String,
    pub user_id: String,
    #[serde(default)]
    pub title: Option<String>,
    pub model: String,
    pub status: String,
}

/// One SSE event from the server. `name` is the SSE `event:` tag
/// (`text_delta`, `tool_call_started`, `done`, etc.); `payload` is the
/// `AgentEvent`-shaped JSON the server emitted.
#[derive(Debug, Clone)]
pub struct RemoteEvent {
    pub name: String,
    pub payload: JsonValue,
}
