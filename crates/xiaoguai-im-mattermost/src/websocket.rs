//! Mattermost WebSocket event stream (stub — v1.3+ full implementation).
//!
//! Mattermost provides a WebSocket API at `wss://{host}/api/v4/websocket`
//! for receiving real-time events without configuring outgoing webhooks.
//! This is useful for "full bot mode" where the bot proactively connects
//! and receives all channel events it has access to.
//!
//! ## Deferred work
//!
//! The following items are deferred to a future milestone:
//!
//! * Authenticate via `{"seq":1,"action":"authentication_challenge","data":{"token":"<bot_token>"}}`
//! * Reconnect with exponential back-off on disconnect.
//! * Dispatch `posted` events into the `ImProvider::parse` pipeline.
//! * Handle `hello` + `status_change` lifecycle events.
//! * Channel mention filtering (`@bot_name` in `message` field).
//!
//! ## Why stub now
//!
//! The outgoing-webhook path (see `incoming.rs`) covers the primary
//! integration use-case with no persistent connection required. WebSocket
//! full-bot mode is opt-in and adds operational complexity (reconnect loop,
//! auth challenge, event filtering). We stub the module now so the crate
//! compiles and downstream code can reference the types without blocking
//! on the implementation.

/// Placeholder type for the future WebSocket event stream driver.
///
/// Construct via `WebSocketDriver::new(base_url, bot_token)` (not yet
/// implemented — will return a [`crate::MattermostError`] until the full
/// implementation lands).
#[derive(Debug)]
pub struct WebSocketDriver {
    _base_url: String,
    _bot_token: String,
}

impl WebSocketDriver {
    /// Build a driver. Currently always returns an error indicating the
    /// feature is not yet implemented.
    ///
    /// # Errors
    ///
    /// Always returns `Err("websocket mode not yet implemented")` until the
    /// v1.3 milestone lands.
    pub fn new(
        base_url: impl Into<String>,
        bot_token: impl Into<String>,
    ) -> Result<Self, &'static str> {
        let _ = base_url.into();
        let _ = bot_token.into();
        Err("websocket mode not yet implemented — use outgoing webhooks instead")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_returns_not_implemented_error() {
        let result = WebSocketDriver::new("wss://mm.example.com", "tok");
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("not yet implemented"));
    }
}
