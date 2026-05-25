//! [`TaskExecutor`] trait + test doubles.
//!
//! Production wiring (the agent runner) implements this trait. Tests use
//! [`MockExecutor`] — a scriptable stub that pops pre-queued responses.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use thiserror::Error;

use crate::card::{KanbanCard, Outcome};

/// Error returned by a failing executor invocation.
#[derive(Debug, Clone, Error)]
pub enum ExecutorError {
    #[error("agent error: {0}")]
    Agent(String),
    #[error("timeout")]
    Timeout,
    #[error("cancelled")]
    Cancelled,
    #[error("internal: {0}")]
    Internal(String),
}

/// Pluggable async executor. Holds the card (with current `attempt`) and must
/// return either a successful [`Outcome`] or an [`ExecutorError`].
///
/// Implementations must be cheaply `Clone`-able (or `Arc`-wrapped) — the pool
/// holds one `Arc<dyn TaskExecutor>` shared across all workers.
#[async_trait]
pub trait TaskExecutor: Send + Sync {
    async fn execute(&self, card: &KanbanCard) -> Result<Outcome, ExecutorError>;
}

// ─── Mock executor ────────────────────────────────────────────────────────────

/// Scriptable test double. Each call to [`execute`] pops the next queued
/// response. Panics if the queue is exhausted (use [`MockExecutor::always_ok`]
/// for infinite-ok behaviour).
#[derive(Clone, Default)]
pub struct MockExecutor {
    queue: Arc<Mutex<Vec<Result<Outcome, ExecutorError>>>>,
    /// When `true` and the queue is empty, return a generic Ok instead of
    /// panicking. Useful for draining large batches.
    infinite_ok: bool,
}

impl MockExecutor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `Ok` for every call, regardless of queue state.
    #[must_use]
    pub fn always_ok() -> Self {
        Self {
            queue: Arc::new(Mutex::new(Vec::new())),
            infinite_ok: true,
        }
    }

    /// Queue a pre-canned `Ok` response.
    pub fn enqueue_ok(&self, summary: impl Into<String>) {
        self.queue
            .lock()
            .unwrap()
            .push(Ok(Outcome::new(summary, serde_json::Value::Null)));
    }

    /// Queue a pre-canned `Err` response.
    pub fn enqueue_err(&self, err: ExecutorError) {
        self.queue.lock().unwrap().push(Err(err));
    }

    /// Number of responses remaining in the queue.
    #[must_use]
    pub fn queue_len(&self) -> usize {
        self.queue.lock().unwrap().len()
    }
}

#[async_trait]
impl TaskExecutor for MockExecutor {
    async fn execute(&self, card: &KanbanCard) -> Result<Outcome, ExecutorError> {
        let mut guard = self.queue.lock().unwrap();
        if let Some(queued) = guard.pop() {
            return queued;
        }
        drop(guard);
        if self.infinite_ok {
            return Ok(Outcome::new(
                format!("mock ok: {}", card.title),
                serde_json::Value::Null,
            ));
        }
        panic!("MockExecutor queue exhausted for card '{}'", card.title);
    }
}
