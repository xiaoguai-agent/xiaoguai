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

impl RemoteClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

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

    /// `POST /v1/sessions/:id/messages` — drain the SSE stream into the
    /// provided sink. The sink receives one `RemoteEvent` per line and may
    /// stop the stream by returning `Err`.
    pub async fn send_message<F>(
        &self,
        session_id: &str,
        content: &str,
        mut on_event: F,
    ) -> Result<()>
    where
        F: FnMut(RemoteEvent) -> Result<()>,
    {
        let resp = self
            .http
            .post(format!(
                "{}/v1/sessions/{session_id}/messages",
                self.base_url
            ))
            .json(&serde_json::json!({ "content": content }))
            .send()
            .await
            .context("POST messages")?;
        require_2xx(&resp)?;

        let mut stream = resp.bytes_stream().eventsource();
        while let Some(ev) = stream.next().await {
            let ev = ev.context("sse event")?;
            let data: JsonValue = serde_json::from_str(&ev.data)
                .with_context(|| format!("decode sse data: {}", ev.data))?;
            let remote = RemoteEvent {
                name: ev.event,
                payload: data,
            };
            let stop = matches!(remote.name.as_str(), "done" | "error");
            on_event(remote)?;
            if stop {
                break;
            }
        }
        Ok(())
    }
}

fn require_2xx(resp: &reqwest::Response) -> Result<()> {
    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else {
        Err(anyhow!("remote returned status {status}"))
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub user_id: String,
    pub tenant_id: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionResponse {
    pub id: String,
    pub tenant_id: String,
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
