//! Token-usage accounting plumbing.
//!
//! Design (matches `docs/plans/2026-05-21-v0.5.2-llm-router.md` T5):
//!
//! - `LlmRouter` wraps each successful stream in a [`UsageRecordingStream`].
//! - When the wrapped stream yields a chunk with `done == true`, the recorder
//!   calls [`UsageSink::record`] once with a [`UsageRecord`] template that
//!   was filled in by the router at call time (tenant / user / session /
//!   provider / model / request id).
//! - Token counts (`prompt_tokens` etc.) stay `None` in v0.5.2 — wiring the
//!   provider-specific extraction (OpenAI's terminal `usage` block, Ollama's
//!   final chunk fields) lands together with the ReAct loop in v0.5.4.
//!
//! `UsageSink::record` is **synchronous** and must not block. Implementations
//! that touch the network or disk should buffer into a channel and flush
//! from a background task (see [`BufferedUsageSink`]).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use futures::stream::StreamExt;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tracing::warn;
use xiaoguai_types::{ProviderId, SessionId, TenantId, UserId};

use crate::backend::{ChatStream, LlmError};
use crate::types::ChatChunk;

#[derive(Debug, Clone)]
pub struct UsageRecord {
    pub ts: DateTime<Utc>,
    pub tenant_id: Option<TenantId>,
    pub user_id: Option<UserId>,
    pub session_id: Option<SessionId>,
    pub provider_id: ProviderId,
    pub model: String,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
    pub request_id: Option<String>,
}

/// Sink interface used by the LLM router. **Must not block** — implementations
/// either drop or enqueue without awaiting.
pub trait UsageSink: Send + Sync {
    fn record(&self, rec: UsageRecord);
}

/// In-memory sink for tests. Keeps every record in an `Arc<Mutex<Vec<…>>>` so
/// callers can inspect what was recorded.
#[derive(Debug, Default, Clone)]
pub struct MemoryUsageSink {
    records: Arc<Mutex<Vec<UsageRecord>>>,
}

impl MemoryUsageSink {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn records(&self) -> Vec<UsageRecord> {
        self.records.lock().clone()
    }
}

impl UsageSink for MemoryUsageSink {
    fn record(&self, rec: UsageRecord) {
        self.records.lock().push(rec);
    }
}

/// Buffered sink: enqueues records into an `mpsc` channel; a background task
/// owned by the caller consumes the receiver and persists in batches.
///
/// `record` uses `try_send`, so an overflowing channel drops the record and
/// logs a warning rather than blocking the user-facing chat stream.
pub struct BufferedUsageSink {
    tx: mpsc::Sender<UsageRecord>,
}

impl BufferedUsageSink {
    /// Build a sink + receiver pair. The caller owns the receiver and is
    /// responsible for spawning a flusher task (see `examples/flusher.rs`
    /// once we ship one; for v0.5.2 just consume in a `while let` loop).
    #[must_use]
    pub fn new(capacity: usize) -> (Self, mpsc::Receiver<UsageRecord>) {
        let (tx, rx) = mpsc::channel(capacity);
        (Self { tx }, rx)
    }
}

impl UsageSink for BufferedUsageSink {
    fn record(&self, rec: UsageRecord) {
        if let Err(e) = self.tx.try_send(rec) {
            warn!(error = %e, "usage channel overflow; dropping record");
        }
    }
}

/// Wrap an inner `ChatStream` so that the first `done: true` chunk triggers
/// exactly one [`UsageSink::record`] call.
pub fn record_on_done(
    inner: ChatStream,
    sink: Arc<dyn UsageSink>,
    template: UsageRecord,
) -> ChatStream {
    let state = (sink, template, false);
    let wrapped = futures::stream::unfold(
        (inner, state),
        |(mut stream, mut state): (ChatStream, (Arc<dyn UsageSink>, UsageRecord, bool))| async move {
            let item: Option<Result<ChatChunk, LlmError>> = stream.next().await;
            match &item {
                Some(Ok(chunk)) if chunk.done && !state.2 => {
                    state.0.record(state.1.clone());
                    state.2 = true;
                }
                _ => {}
            }
            item.map(|i| (i, (stream, state)))
        },
    );
    Box::pin(wrapped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;

    fn dummy_record() -> UsageRecord {
        UsageRecord {
            ts: Utc::now(),
            tenant_id: None,
            user_id: None,
            session_id: None,
            provider_id: ProviderId::from("prov_x".to_string()),
            model: "m".into(),
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
            request_id: None,
        }
    }

    #[tokio::test]
    async fn memory_sink_collects() {
        let sink = MemoryUsageSink::new();
        sink.record(dummy_record());
        sink.record(dummy_record());
        assert_eq!(sink.records().len(), 2);
    }

    #[tokio::test]
    async fn record_on_done_fires_exactly_once() {
        let sink = Arc::new(MemoryUsageSink::new());
        let inner: ChatStream = Box::pin(stream::iter(vec![
            Ok(ChatChunk {
                delta: "He".into(),
                done: false,
            }),
            Ok(ChatChunk {
                delta: "llo".into(),
                done: false,
            }),
            Ok(ChatChunk {
                delta: String::new(),
                done: true,
            }),
        ]));
        let wrapped = record_on_done(inner, sink.clone(), dummy_record());
        let collected: Vec<_> = wrapped.collect().await;
        assert_eq!(collected.len(), 3);
        assert_eq!(sink.records().len(), 1);
    }

    #[tokio::test]
    async fn record_on_done_no_fire_when_no_done() {
        let sink = Arc::new(MemoryUsageSink::new());
        let inner: ChatStream = Box::pin(stream::iter(vec![Ok(ChatChunk {
            delta: "He".into(),
            done: false,
        })]));
        let wrapped = record_on_done(inner, sink.clone(), dummy_record());
        let _: Vec<_> = wrapped.collect().await;
        assert_eq!(sink.records().len(), 0);
    }
}
