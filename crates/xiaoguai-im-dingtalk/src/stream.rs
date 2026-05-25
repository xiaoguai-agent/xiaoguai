//! DingTalk Stream API — WebSocket long-poll inbound client.
//!
//! This module implements the DingTalk "Stream API" which allows tenants that
//! cannot expose a public callback URL to still receive real-time events via
//! a long-lived WebSocket connection.
//!
//! ## Protocol flow
//!
//! 1. POST `https://api.dingtalk.com/v1.0/gateway/connections/open` with
//!    `{clientId, clientSecret, subscriptions, ua}` → `{endpoint, ticket}`.
//! 2. WebSocket connect to `endpoint?ticket=<ticket>`.
//! 3. Receive frames: `{specVersion, type, headers, data}`.
//!    - `type = "CALLBACK"` → call handler, ack with 200.
//!    - `type = "SYSTEM"`, `topic = "disconnect"` → reconnect.
//! 4. Server sends PING every ~8 min; respond PONG.
//! 5. On disconnect: exponential back-off (1 s → 2 s → 4 s → max 60 s).

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

/// Boxed future returned by the event handler.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// ─── Wire types ─────────────────────────────────────────────────────────────

/// Connection negotiation response from DingTalk.
#[derive(Debug, Deserialize)]
pub struct ConnectionResponse {
    pub endpoint: String,
    pub ticket: String,
}

/// A frame arriving over the Stream WebSocket.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamFrame {
    #[serde(default)]
    pub spec_version: String,
    #[serde(rename = "type", default)]
    pub frame_type: String,
    #[serde(default)]
    pub headers: StreamFrameHeaders,
    /// Raw JSON payload — for CALLBACK frames this is the event body.
    #[serde(default)]
    pub data: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamFrameHeaders {
    #[serde(default)]
    pub topic: String,
    #[serde(rename = "messageId", default)]
    pub message_id: String,
    #[serde(default)]
    pub content_type: String,
}

/// Acknowledgement sent back to the DingTalk platform.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamAck {
    pub code: u32,
    pub headers: AckHeaders,
    pub message: String,
    pub data: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AckHeaders {
    pub message_id: String,
    pub content_type: String,
}

impl StreamAck {
    /// Build a success ack for a given `message_id` with `reply_json` data.
    #[must_use]
    pub fn ok(message_id: impl Into<String>, reply_json: impl Into<String>) -> Self {
        Self {
            code: 200,
            headers: AckHeaders {
                message_id: message_id.into(),
                content_type: "application/json".into(),
            },
            message: "OK".into(),
            data: reply_json.into(),
        }
    }
}

// ─── Public API ─────────────────────────────────────────────────────────────

/// High-level types the caller's handler receives and returns.
#[derive(Debug, Clone)]
pub struct InboundMessage {
    /// DingTalk event `data` field (raw JSON string from the frame).
    pub data: String,
    /// The `topic` from the frame headers.
    pub topic: String,
    /// The `messageId` from the frame headers.
    pub message_id: String,
}

/// What the handler must return so the client can ack.
#[derive(Debug, Clone)]
pub struct OutboundReply {
    /// Serialised JSON to put in the ack `data` field.
    /// If `None` the client acks with `"{}"`.
    pub data: Option<String>,
}

impl OutboundReply {
    #[must_use]
    pub fn empty() -> Self {
        Self { data: None }
    }

    #[must_use]
    pub fn with_data(s: impl Into<String>) -> Self {
        Self {
            data: Some(s.into()),
        }
    }
}

// ─── StreamClient ────────────────────────────────────────────────────────────

/// DingTalk Stream API client.
///
/// Call [`StreamClient::run`] to start the long-running WebSocket loop.
/// The call returns only on a fatal (non-retriable) error.
pub struct StreamClient {
    pub client_id: String,
    pub client_secret: String,
    /// Value for the `ua` field sent during connection negotiation.
    pub ua: &'static str,
    /// Override to point at a test server.
    pub gateway_url: String,
}

impl StreamClient {
    const DEFAULT_GATEWAY: &'static str = "https://api.dingtalk.com";

    /// Create a client against the live DingTalk gateway.
    #[must_use]
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            ua: "xiaoguai-stream/1.0",
            gateway_url: Self::DEFAULT_GATEWAY.to_string(),
        }
    }

    /// Override the gateway base URL — used by tests to point at a local mock.
    #[must_use]
    pub fn with_gateway_url(mut self, url: impl Into<String>) -> Self {
        self.gateway_url = url.into();
        self
    }

    /// Start the Stream loop. Runs forever (reconnecting on transient failures)
    /// and returns only on a fatal error (e.g. authentication failure or
    /// handler panic).
    ///
    /// `handler` receives each inbound `CALLBACK` frame and must return an
    /// `OutboundReply`. The reply's `data` goes into the ack frame.
    ///
    /// # Errors
    /// Returns `Err` only for non-retriable failures.
    pub async fn run<H, F>(&self, handler: H) -> Result<()>
    where
        H: Fn(InboundMessage) -> F + Send + Sync + 'static,
        F: Future<Output = OutboundReply> + Send,
    {
        let mut backoff = Duration::from_secs(1);
        loop {
            match self.run_once(&handler).await {
                Ok(Shutdown::GracefulDisconnect) => {
                    info!("dingtalk stream: server requested graceful disconnect — reconnecting");
                }
                Ok(Shutdown::ConnectionClosed) => {
                    info!("dingtalk stream: connection closed — reconnecting in {backoff:?}");
                }
                Err(e) if is_fatal(&e) => {
                    error!("dingtalk stream: fatal error — {e:#}");
                    return Err(e);
                }
                Err(e) => {
                    warn!("dingtalk stream: transient error — {e:#} — reconnecting in {backoff:?}");
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(60));
        }
    }

    // ── Internal helpers ─────────────────────────────────────────────────────

    /// Run one WebSocket session: negotiate, connect, receive-loop. Returns
    /// `Ok(Shutdown::*)` on clean exit and `Err` on anything unexpected.
    async fn run_once<H, F>(&self, handler: &H) -> Result<Shutdown>
    where
        H: Fn(InboundMessage) -> F + Send + Sync,
        F: Future<Output = OutboundReply> + Send,
    {
        // Step 1: negotiate endpoint + ticket.
        let conn = self
            .negotiate()
            .await
            .context("negotiate stream endpoint")?;

        // Step 2: append ticket as query param and connect via WebSocket.
        let ws_url = append_ticket(&conn.endpoint, &conn.ticket);
        info!("dingtalk stream: connecting to {ws_url}");
        let (ws, _response) = connect_async(&ws_url)
            .await
            .with_context(|| format!("ws connect to {ws_url}"))?;
        info!("dingtalk stream: WebSocket connected");

        let (mut sink, mut stream) = ws.split();

        // Step 3: receive loop.
        while let Some(msg) = stream.next().await {
            let msg = msg.context("ws recv")?;
            match msg {
                Message::Text(text) => {
                    debug!("dingtalk stream: frame text len={}", text.len());
                    match serde_json::from_str::<StreamFrame>(&text) {
                        Ok(frame) => {
                            let shutdown = self.handle_frame(frame, handler, &mut sink).await?;
                            if let Some(s) = shutdown {
                                return Ok(s);
                            }
                        }
                        Err(e) => {
                            warn!("dingtalk stream: bad frame JSON — {e}");
                        }
                    }
                }
                Message::Ping(payload) => {
                    debug!("dingtalk stream: received PING, sending PONG");
                    sink.send(Message::Pong(payload))
                        .await
                        .context("send PONG")?;
                }
                Message::Pong(_) | Message::Frame(_) => {}
                Message::Close(frame) => {
                    info!("dingtalk stream: connection closed by server: {frame:?}");
                    return Ok(Shutdown::ConnectionClosed);
                }
                Message::Binary(b) => {
                    debug!("dingtalk stream: unexpected binary frame, len={}", b.len());
                }
            }
        }

        // Stream exhausted — server closed the connection.
        Ok(Shutdown::ConnectionClosed)
    }

    /// Handle a single decoded frame. Returns `Some(Shutdown)` if the
    /// session should end cleanly, `None` to continue the receive loop.
    async fn handle_frame<H, F, Si>(
        &self,
        frame: StreamFrame,
        handler: &H,
        sink: &mut Si,
    ) -> Result<Option<Shutdown>>
    where
        H: Fn(InboundMessage) -> F + Send + Sync,
        F: Future<Output = OutboundReply> + Send,
        Si: SinkExt<Message> + Unpin,
        Si::Error: std::error::Error + Send + Sync + 'static,
    {
        match frame.frame_type.as_str() {
            "CALLBACK" => {
                let msg = InboundMessage {
                    data: frame.data.clone(),
                    topic: frame.headers.topic.clone(),
                    message_id: frame.headers.message_id.clone(),
                };
                let reply = handler(msg).await;
                let data = reply.data.unwrap_or_else(|| "{}".to_string());
                let ack = StreamAck::ok(frame.headers.message_id, data);
                let ack_json = serde_json::to_string(&ack).context("serialise ack")?;
                sink.send(Message::Text(ack_json))
                    .await
                    .map_err(|e| anyhow!("ws send ack: {e}"))?;
            }
            "SYSTEM" => {
                if frame.headers.topic == "disconnect" {
                    return Ok(Some(Shutdown::GracefulDisconnect));
                }
                debug!("dingtalk stream: SYSTEM topic={}", frame.headers.topic);
            }
            other => {
                debug!("dingtalk stream: unknown frame type={other}");
            }
        }
        Ok(None)
    }

    /// POST to the connection-open endpoint to retrieve `{endpoint, ticket}`.
    async fn negotiate(&self) -> Result<ConnectionResponse> {
        let url = format!("{}/v1.0/gateway/connections/open", self.gateway_url);
        let body = serde_json::json!({
            "clientId": self.client_id,
            "clientSecret": self.client_secret,
            "subscriptions": [{"type": "CALLBACK", "topic": "*"}],
            "ua": self.ua,
        });
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .context("build reqwest client")?;
        let resp: JsonValue = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?
            .error_for_status()
            .context("gateway connections/open non-2xx")?
            .json()
            .await
            .context("decode connections/open response")?;

        let endpoint = resp["endpoint"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'endpoint' in connections/open response"))?
            .to_string();
        let ticket = resp["ticket"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'ticket' in connections/open response"))?
            .to_string();

        Ok(ConnectionResponse { endpoint, ticket })
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

#[derive(Debug)]
enum Shutdown {
    GracefulDisconnect,
    ConnectionClosed,
}

fn append_ticket(endpoint: &str, ticket: &str) -> String {
    // Ensure the URL has a path component so the HTTP upgrade request line is
    // `GET /?ticket=... HTTP/1.1` rather than `GET ?ticket=... HTTP/1.1`
    // (the latter is not a valid relative-reference URI and some WS server-side
    // parsers, including tungstenite, reject it).
    let has_path = {
        // strip scheme if present, then check for '/'
        let after_scheme = endpoint
            .find("://")
            .map_or(endpoint, |i| &endpoint[i + 3..]);
        after_scheme.contains('/')
    };
    let sep = if endpoint.contains('?') { '&' } else { '?' };
    if has_path {
        format!("{endpoint}{sep}ticket={ticket}")
    } else {
        format!("{endpoint}/{sep}ticket={ticket}")
    }
}

fn is_fatal(e: &anyhow::Error) -> bool {
    // 401 / 403 from the connections endpoint → don't retry.
    let msg = format!("{e:#}");
    msg.contains("401") || msg.contains("403") || msg.contains("non-2xx")
}

/// Convenience wrapper: build a `StreamClient` and run it.
///
/// This is the entry point called by `xiaoguai-im-gateway` once wired.
/// For now the gateway has not been wired — use `StreamClient::run` directly.
///
/// # Errors
/// Returns an error if the DingTalk stream connection or message loop fails.
pub async fn run_stream<H, F>(
    client_id: impl Into<String>,
    client_secret: impl Into<String>,
    handler: H,
) -> Result<()>
where
    H: Fn(InboundMessage) -> F + Send + Sync + 'static,
    F: Future<Output = OutboundReply> + Send,
{
    StreamClient::new(client_id, client_secret)
        .run(handler)
        .await
}
