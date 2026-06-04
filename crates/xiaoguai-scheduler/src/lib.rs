//! Scheduler crate for xiaoguai (v0.10.x).
//!
//! Three things make this crate worth a separate module instead of
//! living inside `xiaoguai-agent`:
//!
//! 1. **Trigger × Agent.** The community gap (per the v0.9–v0.12
//!    roadmap §3) is platforms that have either strong triggers OR
//!    strong agents but not both. We unify them: a [`Trigger`] is the
//!    only thing the scheduler knows; what runs is a pluggable
//!    [`JobExecutor`] (in v0.10.x the production wiring is the agent
//!    loop, but the trait keeps tests fast and lets v0.11.x's eval
//!    runner reuse the same machinery).
//!
//! 2. **Audit-first by construction.** Every [`JobRun`] writes an
//!    [`xiaoguai_audit::AuditEntry`] via [`AuditAppender`] — `actor =
//!    "scheduler:<job_id>"`. The audit-first console (v0.11.1) gets a
//!    unified chat / IM / scheduled view by reading the `audit_log`,
//!    so this contract is load-bearing.
//!
//! 3. **Retry + push-sink seams.** v0.10.0 shipped the
//!    [`RetryPolicy`] + [`PushSink`] traits with one stub sink
//!    ([`LoggingSink`]); the real sinks (Feishu / Telegram / Email /
//!    chat-ui inbox) land in v0.10.3 against the same trait.
//!
//! v0.10.1 adds the reactive half: the [`Trigger`] enum learns
//! [`Trigger::FileWatch`] / [`Trigger::Webhook`] /
//! [`Trigger::GitPush`] / [`Trigger::DbPoll`] variants, a new
//! [`TriggerSource`] trait + event channel lets external sources
//! push fire requests, and [`JobRunner::run_loop`] merges the
//! scheduled timer and the reactive channel behind one
//! `tokio::select!`. Two concrete sources ship —
//! [`sources::FileWatchSource`] (real `notify` watcher) and
//! [`sources::WebhookSource`] (in-process push handle ready to be
//! fronted by an axum route in xiaoguai-api).
//!
//! v0.10.2 adds the *proactive* third leg:
//! [`Trigger::Proactive`] ticks on its own `interval_secs` but the
//! runner routes each tick through a [`ProactiveChecker`] (cheap-model
//! gate). Only when the checker returns `Some(reason)` does the runner
//! consult the [`BudgetLedger`] for the tenant's per-day push budget;
//! both checks must pass before the executor runs and the
//! reason-carrying [`PushPayload`] reaches the sinks. Roadmap §5.5 is
//! the contract.
//!
//! v0.10.3 fills in real [`PushSink`] implementations under the
//! [`sinks`] module: [`FeishuPushSink`] (reuses
//! `xiaoguai-im-feishu`'s `FeishuClient` + `TokenCache`),
//! [`TelegramPushSink`] (Bot API `sendMessage`),
//! [`EmailPushSink`] (JSON webhook to a relay), and
//! [`InboxPushSink`] (in-memory FIFO drained by the v0.11.1 console).
//! Every real sink enforces the reason-required rule from §5.5
//! through [`PushPayload::require_reason_when_proactive`].
//!
//! Out of scope for v0.10.3: SMTP-native email path (use the webhook
//! relay pattern; see [`sinks::email`] for the rationale).
//! PG-backed repositories + PG budget ledger + PG inbox storage —
//! the in-memory impls here remain the production-shaped contract;
//! the PG sinks land together with the runtime extraction in v0.12.0.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod audit;
pub mod budget;
pub mod composite_executor;
pub mod executor;
pub mod job;
pub mod proactive;
pub mod repository;
pub mod retry;
pub mod runner;
pub mod runtime_executor;
pub mod sink;
pub mod sinks;
pub mod sources;
pub mod sqlite_repository;
pub mod trigger;
pub mod trigger_source;

pub use audit::{AuditAppender, NullAuditAppender, RecordingAuditAppender};
pub use budget::{
    BudgetError, BudgetLedger, InMemoryBudgetLedger, DEFAULT_PROACTIVE_BUDGET_PER_DAY,
};
pub use composite_executor::CompositeExecutor;
pub use executor::{EchoExecutor, ExecutionOutcome, JobExecutor};
pub use job::{JobRun, JobRunStatus, ScheduledJob};
pub use proactive::{
    AlwaysFireChecker, NeverFireChecker, ProactiveChecker, ProactiveCtx, ProactiveError,
    ScriptedChecker,
};
pub use repository::{
    InMemoryJobRepository, InMemoryJobRunRepository, JobRepository, JobRunRepository, RepoError,
};
pub use retry::RetryPolicy;
pub use runner::{JobRunner, RunnerError, RunnerOptions};
pub use runtime_executor::{RuntimeJobExecutor, ScheduledSessionWriter};
pub use sink::{LoggingSink, PushPayload, PushSink, SinkError};
pub use sinks::{
    EmailPushSink, EmailSinkConfig, FeishuPushSink, FeishuSinkConfig, InboxMessage, InboxPushSink,
    TelegramPushSink, TelegramSinkConfig,
};
pub use sources::{FileWatchRoute, FileWatchSource, WebhookRoute, WebhookSource};
pub use sqlite_repository::{SqliteJobRepository, SqliteJobRunRepository};
pub use trigger::{Trigger, TriggerError};
pub use trigger_source::{
    event_channel, EventReceiver, EventSender, SourceError, TriggerEvent, TriggerSource,
    DEFAULT_EVENT_CHANNEL_CAPACITY,
};
