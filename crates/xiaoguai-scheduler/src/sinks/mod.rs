//! Real [`crate::sink::PushSink`] implementations.
//!
//! The trait + the `LoggingSink` stub live in [`crate::sink`]; this
//! module hosts the concrete sinks that v0.10.3 ships against that
//! contract. The split mirrors [`crate::sources`] — one file per real
//! sink, named after the channel it talks to.
//!
//! Sinks shipped in v0.10.3:
//!
//! * [`FeishuPushSink`] — Feishu `im/v1/messages` via
//!   `xiaoguai-im-feishu`'s existing `FeishuClient` + `TokenCache`.
//!   Reuses the v0.7.1 token-cache code path verbatim — do not
//!   duplicate.
//! * [`TelegramPushSink`] — Telegram Bot API `sendMessage` over
//!   `reqwest`. Stateless: bot token + chat id ⇒ one HTTP POST per
//!   delivery.
//! * [`EmailPushSink`] — JSON webhook POST. Pairs naturally with the
//!   "email-relay" microservice pattern most operators already run
//!   (Postmark / Mailgun / a tiny SMTP shim) so the scheduler doesn't
//!   grow an SMTP client dependency for v0.10.3.
//! * [`InboxPushSink`] — in-memory FIFO queue with `pop_all()`, ready
//!   to be drained by the v0.11.1 audit-first console's "Inbox" pane.
//!   Persistence is deferred until the v0.12.0 PG pass.
//!
//! Every real sink enforces the reason-required contract from roadmap
//! §5.5: if `payload.is_proactive && payload.reason.is_empty()`,
//! `deliver` returns [`SinkError::Invalid`] without performing any
//! side effect. The check goes through
//! [`crate::sink::PushPayload::require_reason_when_proactive`] so the
//! rule is one-line + one-place.

pub mod email;
pub mod feishu;
pub mod inbox;
pub mod telegram;

pub use email::{EmailPushSink, EmailSinkConfig};
pub use feishu::{FeishuPushSink, FeishuSinkConfig};
pub use inbox::{InboxMessage, InboxPushSink};
pub use telegram::{TelegramPushSink, TelegramSinkConfig};
