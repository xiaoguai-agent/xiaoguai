//! Trigger sources — where reactive fire events come from.
//!
//! Scheduled triggers ([`Trigger::Cron`] / [`Trigger::Interval`]) are
//! picked up by `JobRepository::list_due` and fired by the runner's
//! timer loop. Reactive triggers ([`Trigger::FileWatch`] /
//! [`Trigger::Webhook`] / [`Trigger::GitPush`] / [`Trigger::DbPoll`])
//! have no wall-clock schedule — they fire when something outside the
//! scheduler (file system, HTTP server, git remote, database row)
//! pokes us.
//!
//! The shared contract is:
//!
//! 1. Each source owns a [`tokio::sync::mpsc::Sender<TriggerEvent>`]
//!    handed in at start-up. It pushes one event per external signal.
//! 2. The runner ([`crate::runner::JobRunner::run_loop`]) merges all
//!    sources behind a single [`EventReceiver`] and uses
//!    `tokio::select!` to react to whichever fires first — including
//!    a scheduled timer tick or a cancellation token.
//! 3. Sources are responsible for their own debounce / dedup. The
//!    runner trusts the event count.
//!
//! v0.10.1 ships two concrete sources:
//!
//! * [`crate::sources::FileWatchSource`] — wraps the `notify` crate
//!   and forwards every changed path to every job whose
//!   `Trigger::FileWatch.path` is an ancestor of (or equal to) the
//!   changed path.
//! * [`crate::sources::WebhookSource`] — in-process; exposes
//!   `push(route_id)` for whatever HTTP plumbing eventually fronts
//!   it. v0.10.1 keeps the actual `/v1/scheduler/webhooks/:route_id`
//!   route out of `xiaoguai-api`; that lands together with operator
//!   wiring (deferred from v0.10.0).
//!
//! `GitPushSource` and `DbPollSource` are intentionally not present
//! in v0.10.1 — the [`Trigger`] data variants ship so persisted job
//! rows are forward-compatible, but the polling/webhook adapters
//! land in v0.10.1.x once we have a real consumer asking for them.
//!
//! [`Trigger`]: crate::trigger::Trigger

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use thiserror::Error;
use tokio::sync::mpsc;

/// One fire request from a reactive source to the runner.
///
/// `detail` is opaque to the runner — sources may stash anything
/// useful for debugging (`{"changed_path": ".../foo.md"}` for the
/// file watcher, `{"remote_addr": "1.2.3.4"}` for the webhook). It's
/// written into the `audit_log.details` JSONB alongside the standard
/// `run_id`/`attempt`/`status` so the audit-first console (v0.11.1)
/// can show "fired because file X changed at T".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerEvent {
    pub job_id: String,
    pub fired_at: DateTime<Utc>,
    pub detail: serde_json::Value,
}

impl TriggerEvent {
    #[must_use]
    pub fn new(job_id: impl Into<String>) -> Self {
        Self {
            job_id: job_id.into(),
            fired_at: Utc::now(),
            detail: serde_json::Value::Null,
        }
    }

    #[must_use]
    pub fn with_detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = detail;
        self
    }
}

/// Default capacity for the runner-side event channel.
///
/// The runner drains the channel as fast as it can fire jobs, so
/// this is mostly a back-pressure cushion against bursty sources
/// (a `cp -r` over a 1k-file directory under a file watcher).
pub const DEFAULT_EVENT_CHANNEL_CAPACITY: usize = 256;

/// Receiver end of the shared event channel. Owned by the runner.
pub type EventReceiver = mpsc::Receiver<TriggerEvent>;

/// Sender end of the shared event channel. Handed to every
/// [`TriggerSource`] and to ad-hoc producers (e.g. the webhook HTTP
/// handler once xiaoguai-api wires it in).
pub type EventSender = mpsc::Sender<TriggerEvent>;

/// Construct a new (sender, receiver) pair with the default capacity.
#[must_use]
pub fn event_channel() -> (EventSender, EventReceiver) {
    mpsc::channel(DEFAULT_EVENT_CHANNEL_CAPACITY)
}

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("source backend: {0}")]
    Backend(String),
    #[error("source already started")]
    AlreadyStarted,
}

/// Contract for anything that produces [`TriggerEvent`]s.
///
/// A source typically spawns a background task in `start` and
/// returns immediately. The task owns the cloned [`EventSender`] and
/// is expected to exit cleanly when every sender side is dropped
/// (which happens when the runner shuts down and drops its receiver
/// — the `send` calls will error with `SendError`, and the source
/// task should treat that as "we're done").
#[async_trait]
pub trait TriggerSource: Send + Sync {
    /// Stable identifier for diagnostics (e.g. `"file_watch"`,
    /// `"webhook"`). Not used for routing — the runner uses the job's
    /// own `Trigger` variant for that.
    fn id(&self) -> &'static str;

    /// Spawn whatever the source needs to spawn. Most sources use
    /// `tx.clone()` and move it into a background task.
    async fn start(&self, tx: EventSender) -> Result<(), SourceError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn event_channel_round_trip() {
        let (tx, mut rx) = event_channel();
        let ev = TriggerEvent::new("j1").with_detail(serde_json::json!({"k": 1}));
        tx.send(ev.clone()).await.unwrap();
        let got = rx.recv().await.unwrap();
        assert_eq!(got.job_id, "j1");
        assert_eq!(got.detail, serde_json::json!({"k": 1}));
    }

    #[tokio::test]
    async fn event_constructor_sets_fired_at() {
        let ev = TriggerEvent::new("j1");
        let now = Utc::now();
        let delta = (now - ev.fired_at).num_seconds().abs();
        assert!(delta < 5, "fired_at should be close to now");
    }
}
