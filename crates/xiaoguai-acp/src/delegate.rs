//! The decoupling seam between the protocol/transport and the agent.
//!
//! [`server`](crate::server) knows nothing about how a prompt turn is executed;
//! it owns sessions, cancellation, and framing. An [`AcpDelegate`] does the
//! actual turn — emitting `session/update` notifications through an
//! [`UpdateSink`] and returning the ACP [`StopReason`](crate::acp::StopReason).
//! Tests drive the server with a deterministic stub delegate; the CLI supplies
//! [`RuntimeDelegate`](crate::RuntimeDelegate).

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::acp::{SessionUpdate, StopReason};

/// A non-blocking channel the delegate pushes `SessionUpdate`s into; the server
/// drains it and writes each as a `session/update` notification, in order,
/// interleaved with nothing else for that turn.
#[derive(Clone)]
pub struct UpdateSink {
    tx: mpsc::UnboundedSender<SessionUpdate>,
}

impl UpdateSink {
    /// Build a sink + its receiver. The server keeps the receiver.
    #[must_use]
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<SessionUpdate>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    /// Emit one update. A send only fails if the server stopped draining (turn
    /// torn down); that is benign and intentionally ignored.
    pub fn send(&self, update: SessionUpdate) {
        let _ = self.tx.send(update);
    }
}

/// Drives a single ACP prompt turn for one session.
#[async_trait]
pub trait AcpDelegate: Send + Sync {
    /// Run one prompt turn.
    ///
    /// * `session_id` — the ACP session the prompt belongs to.
    /// * `prompt_text` — the concatenated text of the prompt's content blocks.
    /// * `sink` — emit assistant/tool `SessionUpdate`s here as they happen.
    /// * `cancel` — fires when the client sends `session/cancel`; the turn
    ///   should unwind and return [`StopReason::Cancelled`].
    ///
    /// Returns the stop reason that closes the turn. Implementations surface
    /// their own errors as updates (e.g. a warning chunk) and still return a
    /// stop reason — the RPC itself only fails for protocol faults.
    async fn prompt(
        &self,
        session_id: &str,
        prompt_text: String,
        sink: UpdateSink,
        cancel: CancellationToken,
    ) -> StopReason;
}
