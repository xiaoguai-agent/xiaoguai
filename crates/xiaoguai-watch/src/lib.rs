//! `xiaoguai-watch` — declarative active-wakeup watchers.
//!
//! This crate provides the watch DSL that lets an agent wake unprompted when
//! patterns appear in data streams — the third tier of the
//! `passive → reactive → proactive → active-wakeup` ladder described in
//! Sivulka's "institutional AI" thesis (see design workspace docs).
//!
//! ## Quick start
//!
//! ```no_run
//! use xiaoguai_watch::{WatchRunner, WatchSpec, WatchSourceSpec, WatchSchedule,
//!                       ActionRef, InMemorySource, DedupCache};
//! use std::time::Duration;
//! use serde_json::json;
//!
//! # #[tokio::main]
//! # async fn main() {
//! let spec = WatchSpec {
//!     id: "ar-aging".into(),
//!     source: WatchSourceSpec::Sql {
//!         query: "SELECT tenant_id, dso FROM ar_aging WHERE dso > 60".into(),
//!     },
//!     schedule: WatchSchedule::IntervalSecs { secs: 86400 },
//!     on_match: ActionRef {
//!         action: "notify".into(),
//!         target: Some("ops-channel".into()),
//!         params: serde_json::Map::new(),
//!     },
//! };
//!
//! let source = InMemorySource::new(vec![
//!     serde_json::from_value(json!({"tenant_id": "acme", "dso": 72})).unwrap(),
//! ]);
//!
//! let dedup = DedupCache::new(1_000, Duration::from_secs(86400));
//! let mut runner = WatchRunner::with_dedup(dedup);
//! runner.register(spec, source);
//! let mut rx = runner.run();
//!
//! if let Some(event) = rx.recv().await {
//!     println!("WatchEvent: {:?}", event.payload);
//! }
//! # }
//! ```
//!
//! ## Modules
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`spec`] | [`WatchSpec`] — declarative watcher definition (YAML/JSON) |
//! | [`source`] | [`WatchSource`] trait + [`SqlSource`], [`HttpSource`], [`InMemorySource`] |
//! | [`dedup`] | [`DedupCache`] — SHA-256 + moka TTL deduplication |
//! | [`runner`] | [`WatchRunner`] + [`WatchEvent`] |

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod dedup;
pub mod runner;
pub mod source;
pub mod spec;

// Public re-exports — the full public API surface.
pub use dedup::DedupCache;
pub use runner::{WatchEvent, WatchRunner};
pub use source::{HttpSource, InMemorySource, Match, SourceError, SqlSource, WatchSource};
pub use spec::{ActionRef, WatchSchedule, WatchSourceSpec, WatchSpec};
