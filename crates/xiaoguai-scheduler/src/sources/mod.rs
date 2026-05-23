//! Concrete [`crate::trigger_source::TriggerSource`] implementations.
//!
//! v0.10.1 ships:
//!
//! * [`FileWatchSource`] — wraps the `notify` crate.
//! * [`WebhookSource`] — in-process push handle keyed by `route_id`;
//!   the HTTP route in `xiaoguai-api` is wired in a later tag.
//!
//! `GitPushSource` / `DbPollSource` are intentionally absent — the
//! [`Trigger`](crate::trigger::Trigger) data variants ship in
//! v0.10.1 so persisted job rows are forward-compatible, but the
//! polling adapters wait for v0.10.1.x.

pub mod file_watch;
pub mod webhook;

pub use file_watch::{FileWatchRoute, FileWatchSource};
pub use webhook::{WebhookRoute, WebhookSource};
