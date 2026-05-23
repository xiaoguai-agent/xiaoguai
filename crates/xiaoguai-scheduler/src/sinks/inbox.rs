//! Inbox push sink — in-memory FIFO queue.
//!
//! v0.10.3 keeps this in-process so the v0.11.1 audit-first console's
//! "Inbox" pane can drain it via `pop_all()` and render whatever
//! arrived since the last poll. Persistence across restarts is
//! deferred until the v0.12.0 PG pass — the contract (a FIFO of
//! `InboxMessage`s scoped per tenant) is the same either way; only
//! the storage swaps.
//!
//! Unlike the network sinks the inbox has no transient-failure mode
//! to worry about; `deliver` only ever returns
//! [`SinkError::Invalid`] for the reason-required violation, and only
//! ever after the proactive guard fires.

use std::collections::VecDeque;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::sink::{PushPayload, PushSink, SinkError};

/// One inbox entry. The console reads the payload directly; the
/// `enqueued_at` is added by the sink so the console can sort by
/// arrival even when several pushes share the same `fired_at`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InboxMessage {
    pub enqueued_at: DateTime<Utc>,
    pub payload: PushPayload,
}

pub struct InboxPushSink {
    id: String,
    queue: Mutex<VecDeque<InboxMessage>>,
}

impl InboxPushSink {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            queue: Mutex::new(VecDeque::new()),
        }
    }

    /// Drain every queued message, leaving the inbox empty. Returned
    /// in arrival order (oldest first).
    #[must_use]
    pub fn pop_all(&self) -> Vec<InboxMessage> {
        let mut g = self.queue.lock();
        g.drain(..).collect()
    }

    /// Non-destructive count for diagnostics + tests.
    #[must_use]
    pub fn len(&self) -> usize {
        self.queue.lock().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queue.lock().is_empty()
    }
}

impl Default for InboxPushSink {
    fn default() -> Self {
        Self::new("inbox")
    }
}

impl std::fmt::Debug for InboxPushSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboxPushSink")
            .field("id", &self.id)
            .field("queue_len", &self.queue.lock().len())
            .finish()
    }
}

#[async_trait]
impl PushSink for InboxPushSink {
    fn id(&self) -> &str {
        &self.id
    }

    async fn deliver(&self, payload: &PushPayload) -> Result<(), SinkError> {
        payload.require_reason_when_proactive()?;
        self.queue.lock().push_back(InboxMessage {
            enqueued_at: Utc::now(),
            payload: payload.clone(),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(is_proactive: bool, reason: &str, run_id: i64) -> PushPayload {
        PushPayload {
            job_id: "j1".into(),
            run_id,
            tenant_id: Some("t1".into()),
            status: "succeeded".into(),
            fired_at: Utc::now(),
            output_preview: None,
            error_message: None,
            reason: reason.into(),
            is_proactive,
        }
    }

    #[tokio::test]
    async fn proactive_with_empty_reason_is_refused() {
        let sink = InboxPushSink::new("inbox");
        let err = sink.deliver(&payload(true, "", 1)).await.unwrap_err();
        assert!(matches!(err, SinkError::Invalid(_)));
        assert_eq!(sink.len(), 0);
    }

    #[tokio::test]
    async fn pop_all_drains_in_order() {
        let sink = InboxPushSink::new("inbox");
        sink.deliver(&payload(false, "", 1)).await.unwrap();
        sink.deliver(&payload(true, "wake up", 2)).await.unwrap();
        sink.deliver(&payload(false, "", 3)).await.unwrap();
        assert_eq!(sink.len(), 3);
        let drained = sink.pop_all();
        assert_eq!(drained.len(), 3);
        let ids: Vec<i64> = drained.iter().map(|m| m.payload.run_id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
        assert!(sink.is_empty());
    }

    #[tokio::test]
    async fn id_matches_constructor() {
        let sink = InboxPushSink::new("user-1-inbox");
        assert_eq!(sink.id(), "user-1-inbox");
    }
}
