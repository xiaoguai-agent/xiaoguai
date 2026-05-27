//! Slack Socket Mode client (optional).
//!
//! Socket Mode lets Slack deliver events over a long-lived WebSocket
//! connection instead of requiring a public HTTPS endpoint. Useful for
//! tenants behind NAT or corporate firewalls.
//!
//! Flow:
//! 1. Call `apps.connections.open` with an **App-Level Token** (`xapp-…`)
//!    to obtain a short-lived WSS URL.
//! 2. Connect with `tokio-tungstenite`.
//! 3. For every incoming JSON frame:
//!    - ACK immediately with `{"envelope_id":"<id>"}`.
//!    - If `type == "events_api"`, extract the inner `payload` and forward
//!      it to `crate::inbound::parse_event`.
//!    - If `type == "hello"`, log and continue (Slack handshake).
//!    - If `type == "disconnect"`, reconnect after a short back-off.
//! 4. Re-open the WSS URL every ~10 min (Slack closes it after ~30 min).
//!
//! This module is **optional** — gate it behind the `socket-mode` Cargo
//! feature if you want to keep the default binary lean.
//!
//! The handler callback receives the parsed [`ImEvent`] exactly as the
//! HTTP Events API path does, so application logic is provider-agnostic.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{debug, error, info, warn};

use xiaoguai_im_gateway::{ImEvent, ProviderError};

use crate::inbound::{parse_event, SocketModePayload};

/// Reconnect back-off after a `disconnect` frame or network error.
const RECONNECT_BACKOFF: Duration = Duration::from_secs(5);

/// Refresh the WSS URL after this many seconds (Slack closes after ~30 min).
const WSS_REFRESH_INTERVAL: Duration = Duration::from_secs(9 * 60);

/// HTTP timeout for the `apps.connections.open` call.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

// ── apps.connections.open ─────────────────────────────────────────────────────

/// Call `apps.connections.open` and return the WSS URL.
///
/// # Errors
/// Returns [`ProviderError::Transport`] on HTTP or JSON errors, or if the
/// Slack API returns `ok: false`.
pub async fn open_connection(app_token: &str, base_url: &str) -> Result<String, ProviderError> {
    #[derive(Deserialize)]
    struct Resp {
        ok: bool,
        #[serde(default)]
        url: Option<String>,
        #[serde(default)]
        error: Option<String>,
    }

    let client = reqwest::Client::builder()
        .timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|e| ProviderError::Transport(format!("build reqwest: {e}")))?;

    let url = format!("{base_url}/api/apps.connections.open");

    let raw = client
        .post(&url)
        .bearer_auth(app_token)
        .json(&json!({}))
        .send()
        .await
        .map_err(|e| ProviderError::Transport(format!("apps.connections.open send: {e}")))?;

    let resp: Resp = raw
        .json()
        .await
        .map_err(|e| ProviderError::Transport(format!("apps.connections.open decode: {e}")))?;

    if !resp.ok {
        return Err(ProviderError::Transport(format!(
            "apps.connections.open error: {:?}",
            resp.error.unwrap_or_else(|| "unknown".into())
        )));
    }

    resp.url
        .ok_or_else(|| ProviderError::Transport("apps.connections.open: missing url".into()))
}

// ── main loop ─────────────────────────────────────────────────────────────────

/// Run the Socket Mode event loop.
///
/// Calls `handler` for every parsed [`ImEvent`]. Bot messages and Slack
/// retry frames are already filtered by [`parse_event`].
///
/// This function runs until `handler` returns `Err` or an unrecoverable
/// transport error occurs. Reconnect on disconnect frames is handled
/// internally.
///
/// `base_url` is injectable so tests can point at a mock server.
///
/// # Errors
/// Returns [`ProviderError::Transport`] only on fatal, unrecoverable errors
/// (e.g. failure to obtain the initial WSS URL). Transient disconnects
/// cause an internal reconnect loop.
pub async fn run<F, Fut>(
    app_token: &str,
    base_url: &str,
    mut handler: F,
) -> Result<(), ProviderError>
where
    F: FnMut(ImEvent) -> Fut + Send,
    Fut: std::future::Future<Output = Result<(), ProviderError>> + Send,
{
    loop {
        let wss_url = open_connection(app_token, base_url).await?;
        info!(%wss_url, "socket_mode: connecting");

        let (ws_stream, _) = connect_async(&wss_url)
            .await
            .map_err(|e| ProviderError::Transport(format!("websocket connect: {e}")))?;

        let (mut write, mut read) = ws_stream.split();
        let started = std::time::Instant::now();

        loop {
            // Refresh the WSS URL before Slack closes it.
            if started.elapsed() > WSS_REFRESH_INTERVAL {
                info!("socket_mode: refreshing WSS URL");
                break;
            }

            let msg = match read.next().await {
                Some(Ok(m)) => m,
                Some(Err(e)) => {
                    warn!("socket_mode: ws error: {e}");
                    tokio::time::sleep(RECONNECT_BACKOFF).await;
                    break;
                }
                None => {
                    info!("socket_mode: stream closed, reconnecting");
                    tokio::time::sleep(RECONNECT_BACKOFF).await;
                    break;
                }
            };

            let text = match msg {
                Message::Text(t) => t.to_string(),
                Message::Ping(p) => {
                    let _ = write.send(Message::Pong(p)).await;
                    continue;
                }
                Message::Close(_) => {
                    info!("socket_mode: close frame received, reconnecting");
                    tokio::time::sleep(RECONNECT_BACKOFF).await;
                    break;
                }
                _ => continue,
            };

            let frame: SocketModePayload = match serde_json::from_str(&text) {
                Ok(f) => f,
                Err(e) => {
                    warn!("socket_mode: failed to decode frame: {e}");
                    continue;
                }
            };

            match frame.kind.as_str() {
                "hello" => {
                    info!("socket_mode: hello received — connected");
                }
                "disconnect" => {
                    info!("socket_mode: disconnect frame, reconnecting");
                    let ack = json!({"envelope_id": frame.envelope_id});
                    let _ = write.send(Message::Text(ack.to_string().into())).await;
                    tokio::time::sleep(RECONNECT_BACKOFF).await;
                    break;
                }
                "events_api" => {
                    // ACK immediately so Slack doesn't retry.
                    if let Some(ref eid) = frame.envelope_id {
                        let ack = json!({"envelope_id": eid});
                        if let Err(e) = write.send(Message::Text(ack.to_string().into())).await {
                            error!("socket_mode: failed to ACK {eid}: {e}");
                        } else {
                            debug!(%eid, "socket_mode: ACK sent");
                        }
                    }
                    // Extract + parse the inner payload.
                    let payload_bytes = if let Some(ref v) = frame.payload {
                        serde_json::to_vec(v).unwrap_or_default()
                    } else {
                        warn!("socket_mode: events_api frame missing payload");
                        continue;
                    };
                    match parse_event(&payload_bytes, None) {
                        Ok(event) => handler(event).await?,
                        Err(e) => {
                            warn!("socket_mode: parse error: {e}");
                        }
                    }
                }
                other => {
                    debug!(%other, "socket_mode: unhandled frame type");
                }
            }
        }
    }
}

/// Parse a raw Socket Mode JSON frame for unit testing without a live
/// WebSocket connection.
///
/// # Errors
///
/// Returns [`serde_json::Error`] if `raw` is not valid JSON or does not match
/// the [`SocketModePayload`] schema.
pub fn parse_socket_frame(raw: &str) -> Result<SocketModePayload, serde_json::Error> {
    serde_json::from_str(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hello_frame() {
        let raw = r#"{"type":"hello","num_connections":1,"debug_info":{}}"#;
        let f = parse_socket_frame(raw).unwrap();
        assert_eq!(f.kind, "hello");
        assert!(f.envelope_id.is_none());
    }

    #[test]
    fn parse_events_api_frame_contains_payload() {
        let raw = r#"{
            "type": "events_api",
            "envelope_id": "env-001",
            "payload": {
                "type": "event_callback",
                "team_id": "T1",
                "event_id": "Ev1",
                "event": {
                    "type": "message",
                    "channel": "C1",
                    "user": "U1",
                    "text": "hi from socket mode",
                    "ts": "1716355200.000100"
                }
            }
        }"#;
        let f = parse_socket_frame(raw).unwrap();
        assert_eq!(f.kind, "events_api");
        assert_eq!(f.envelope_id.as_deref(), Some("env-001"));

        // Re-parse the inner payload through the inbound parser.
        let payload_bytes = serde_json::to_vec(&f.payload.unwrap()).unwrap();
        match parse_event(&payload_bytes, None).unwrap() {
            ImEvent::Message(m) => {
                assert_eq!(m.text, "hi from socket mode");
                assert_eq!(m.user_external_id, "U1");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_disconnect_frame() {
        let raw = r#"{"type":"disconnect","reason":"refresh_requested","envelope_id":"env-002"}"#;
        let f = parse_socket_frame(raw).unwrap();
        assert_eq!(f.kind, "disconnect");
        assert_eq!(f.envelope_id.as_deref(), Some("env-002"));
    }
}
