//! Long-poll loop using `getUpdates` — an alternative to webhook delivery.
//!
//! Instead of registering a URL with Telegram, the bot calls `getUpdates`
//! repeatedly with an `offset` equal to the highest `update_id` seen + 1.
//! Telegram holds the connection open up to `timeout` seconds if there are
//! no pending updates (long-polling), then returns whatever arrived.
//!
//! The loop state is captured in [`LongPollState`], which is kept separate
//! from the network call so unit tests can drive it without real I/O.
//!
//! Usage:
//! ```no_run
//! use std::sync::Arc;
//! use xiaoguai_im_telegram::long_poll::{LongPollState, run_long_poll};
//! use xiaoguai_im_telegram::outbound::HttpTelegramClient;
//!
//! # async fn example() {
//! let client = Arc::new(HttpTelegramClient::new("BOT_TOKEN").unwrap());
//! let mut state = LongPollState::default();
//! run_long_poll(client, &mut state, 30, |update| {
//!     println!("received update_id={}", update.update_id);
//! }).await;
//! # }
//! ```

use std::sync::Arc;

use serde_json::json;
use tracing::instrument;
use xiaoguai_im_gateway::ProviderError;

use crate::outbound::TelegramClient;
use crate::types::Update;

/// Mutable long-poll state: tracks the offset to send on the next call.
///
/// The offset starts at `0` (Telegram returns all pending updates) and
/// advances after each batch so already-seen updates are acknowledged.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LongPollState {
    /// Next offset to send. `0` means "from the oldest pending update".
    pub offset: i64,
}

impl LongPollState {
    /// Advance the offset past `update_id` so Telegram acks that update.
    pub fn advance(&mut self, update_id: i64) {
        if update_id + 1 > self.offset {
            self.offset = update_id + 1;
        }
    }
}

/// Fetch one batch of updates using `getUpdates`.
///
/// Returns the raw list of [`Update`]s.  Callers are responsible for calling
/// [`LongPollState::advance`] for each update they have processed.
///
/// `timeout_secs` is the long-poll timeout passed to Telegram (0 = short
/// poll; typically 30 in production).
///
/// # Errors
/// Returns `ProviderError::Transport` on network or decode failures, or if
/// the Telegram API returns `ok: false`.
pub async fn fetch_updates(
    client: &dyn TelegramClient,
    state: &LongPollState,
    timeout_secs: u32,
) -> Result<Vec<Update>, ProviderError> {
    let body = json!({
        "offset": state.offset,
        "timeout": timeout_secs,
        "allowed_updates": ["message", "edited_message", "callback_query"],
    });
    let result = client.call("getUpdates", body).await?;
    let updates: Vec<Update> = serde_json::from_value(result)
        .map_err(|e| ProviderError::Malformed(format!("decode getUpdates result: {e}")))?;
    Ok(updates)
}

/// High-level long-poll loop. Calls `getUpdates` in a loop, forwarding each
/// update to `on_update` and advancing the offset automatically.
///
/// The loop runs until the process is shut down (it never returns normally).
/// Errors are logged and the loop retries after a short back-off.
#[instrument(skip(client, state, on_update))]
pub async fn run_long_poll<F>(
    client: Arc<dyn TelegramClient>,
    state: &mut LongPollState,
    timeout_secs: u32,
    mut on_update: F,
) where
    F: FnMut(Update),
{
    loop {
        match fetch_updates(client.as_ref(), state, timeout_secs).await {
            Ok(updates) => {
                for update in updates {
                    state.advance(update.update_id);
                    on_update(update);
                }
            }
            Err(e) => {
                tracing::warn!(?e, "getUpdates error, retrying");
                // Small back-off to avoid hammering the API on persistent
                // failures. In production callers should inject a
                // `CancellationToken` — that extension point is deferred
                // to v1.3.0.
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbound::TelegramClient;
    use crate::types::GetUpdatesResult;
    use parking_lot::Mutex;
    use std::sync::Arc;

    // ---------------------------------------------------------------------------
    // Test double
    // ---------------------------------------------------------------------------

    struct FakeClient {
        /// Each call returns the next entry; wraps around when exhausted.
        responses: Mutex<std::collections::VecDeque<Result<serde_json::Value, String>>>,
        calls: Mutex<Vec<serde_json::Value>>,
    }

    impl FakeClient {
        fn with_responses(
            responses: Vec<Result<serde_json::Value, String>>,
        ) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(responses.into_iter().collect()),
                calls: Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait::async_trait]
    impl TelegramClient for FakeClient {
        async fn call(
            &self,
            _method: &str,
            body: serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            self.calls.lock().push(body);
            match self.responses.lock().pop_front() {
                Some(Ok(v)) => Ok(v),
                Some(Err(e)) => Err(ProviderError::Transport(e)),
                None => Ok(serde_json::json!([])), // empty batch when exhausted
            }
        }
    }

    // Helpers to build API responses.
    fn updates_response(updates: &[serde_json::Value]) -> serde_json::Value {
        serde_json::json!(updates)
    }

    fn make_update(update_id: i64, text: &str) -> serde_json::Value {
        serde_json::json!({
            "update_id": update_id,
            "message": {
                "message_id": update_id * 10,
                "from": {"id": 1, "first_name": "User"},
                "chat": {"id": 100, "type": "private"},
                "text": text
            }
        })
    }

    // ---------------------------------------------------------------------------
    // LongPollState offset tracking
    // ---------------------------------------------------------------------------

    #[test]
    fn advance_moves_offset_past_seen_update() {
        let mut state = LongPollState::default();
        assert_eq!(state.offset, 0);
        state.advance(5);
        assert_eq!(state.offset, 6);
    }

    #[test]
    fn advance_does_not_regress_offset() {
        let mut state = LongPollState { offset: 10 };
        state.advance(3); // older than current offset — must not regress
        assert_eq!(state.offset, 10);
    }

    #[test]
    fn advance_handles_consecutive_updates() {
        let mut state = LongPollState::default();
        state.advance(0);
        assert_eq!(state.offset, 1);
        state.advance(1);
        assert_eq!(state.offset, 2);
        state.advance(9);
        assert_eq!(state.offset, 10);
    }

    // ---------------------------------------------------------------------------
    // fetch_updates
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn fetch_updates_empty_response() {
        let client = FakeClient::with_responses(vec![Ok(updates_response(&[]))]);
        let state = LongPollState::default();
        let updates = fetch_updates(client.as_ref(), &state, 1).await.unwrap();
        assert!(updates.is_empty());
    }

    #[tokio::test]
    async fn fetch_updates_sends_offset_in_request() {
        let client = FakeClient::with_responses(vec![Ok(updates_response(&[]))]);
        let state = LongPollState { offset: 42 };
        fetch_updates(client.as_ref(), &state, 5).await.unwrap();
        let call_body = &client.calls.lock()[0];
        assert_eq!(call_body["offset"], 42);
        assert_eq!(call_body["timeout"], 5);
    }

    #[tokio::test]
    async fn fetch_updates_returns_parsed_updates() {
        let client = FakeClient::with_responses(vec![Ok(updates_response(&[
            make_update(100, "hello"),
            make_update(101, "world"),
        ]))]);
        let state = LongPollState::default();
        let updates = fetch_updates(client.as_ref(), &state, 1).await.unwrap();
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].update_id, 100);
        assert_eq!(updates[1].update_id, 101);
    }

    #[tokio::test]
    async fn offset_advances_correctly_across_multiple_batches() {
        // Simulate two consecutive getUpdates calls:
        // Batch 1: update_ids [10, 11, 12]
        // Batch 2: update_ids [13]
        let client = FakeClient::with_responses(vec![
            Ok(updates_response(&[
                make_update(10, "a"),
                make_update(11, "b"),
                make_update(12, "c"),
            ])),
            Ok(updates_response(&[make_update(13, "d")])),
        ]);

        let mut state = LongPollState::default();

        // First batch.
        let updates = fetch_updates(client.as_ref(), &state, 1).await.unwrap();
        for u in &updates {
            state.advance(u.update_id);
        }
        assert_eq!(state.offset, 13, "offset after first batch");

        // Second batch — offset 13 must be sent.
        let updates = fetch_updates(client.as_ref(), &state, 1).await.unwrap();
        for u in &updates {
            state.advance(u.update_id);
        }
        assert_eq!(state.offset, 14, "offset after second batch");

        // Verify the second call used offset=13.
        let calls = client.calls.lock();
        assert_eq!(calls[1]["offset"], 13);
    }

    #[tokio::test]
    async fn fetch_updates_propagates_transport_error() {
        let client =
            FakeClient::with_responses(vec![Err("network failure".to_string())]);
        let state = LongPollState::default();
        let err = fetch_updates(client.as_ref(), &state, 1).await.unwrap_err();
        assert!(matches!(err, ProviderError::Transport(_)));
    }

    // ---------------------------------------------------------------------------
    // GetUpdatesResult deserialisation (envelope layer)
    // ---------------------------------------------------------------------------

    #[test]
    fn deserialise_get_updates_result_ok() {
        let json = serde_json::json!({
            "ok": true,
            "result": [
                {
                    "update_id": 1,
                    "message": {
                        "message_id": 10,
                        "from": {"id": 5, "first_name": "X"},
                        "chat": {"id": 20, "type": "private"},
                        "text": "hi"
                    }
                }
            ]
        });
        let r: GetUpdatesResult = serde_json::from_value(json).unwrap();
        assert!(r.ok);
        let results = r.result.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].update_id, 1);
    }

    #[test]
    fn deserialise_get_updates_result_err() {
        let json = serde_json::json!({
            "ok": false,
            "description": "Unauthorized",
            "error_code": 401
        });
        let r: GetUpdatesResult = serde_json::from_value(json).unwrap();
        assert!(!r.ok);
        assert_eq!(r.description.as_deref(), Some("Unauthorized"));
        assert!(r.result.is_none());
    }
}
