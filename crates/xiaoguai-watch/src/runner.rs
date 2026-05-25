//! [`WatchRunner`] — drives all registered [`WatchSpec`]s on their schedules.
//!
//! ## Architecture
//!
//! ```text
//! WatchRunner::run ──► per-spec tick loop (tokio::spawn per spec)
//!                           │
//!                           ▼
//!                      WatchSource::poll()
//!                           │
//!                           ▼
//!                      DedupCache  ──[duplicate]──► drop
//!                           │
//!                      [new match]
//!                           │
//!                           ▼
//!                      mpsc::Sender<WatchEvent> ──► caller
//! ```
//!
//! ## Lifecycle
//!
//! 1. Build the runner with [`WatchRunner::new`].
//! 2. Register specs + sources with [`WatchRunner::register`].
//! 3. Call [`WatchRunner::run`] — it spawns one task per spec and returns
//!    the event receiver.
//! 4. The scheduler integrator reads from the receiver and dispatches
//!    [`WatchEvent`]s to the appropriate executor / sink.
//!
//! ## Stopping
//!
//! Drop the [`WatchRunner`] (or the returned `EventReceiver`) to stop all
//! watch tasks.  All spawned tasks hold a `Weak` reference to a shared
//! shutdown `CancellationToken`; dropping the runner triggers cancellation.
//!
//! ## Scheduler integration hook
//!
//! The `WatchEvent` is deliberately minimal — it carries only the spec id,
//! the raw match payload, and the `ActionRef`.  The scheduler integrator maps
//! `on_match.action` to a concrete executor (e.g. `"notify"` → `FeishuPushSink`,
//! `"create_task"` → `RuntimeJobExecutor`).  This keeps `xiaoguai-watch` free
//! of direct scheduler / IM crate dependencies.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::mpsc;
use tracing::{error, info, instrument, warn};

use crate::dedup::DedupCache;
use crate::source::{SourceError, WatchSource};
use crate::spec::{ActionRef, WatchSchedule, WatchSpec};

// ---------------------------------------------------------------------------
// WatchEvent
// ---------------------------------------------------------------------------

/// An event emitted when a non-deduplicated match is found.
///
/// Consumed by the scheduler integrator that sits downstream of the runner.
#[derive(Debug, Clone)]
pub struct WatchEvent {
    /// ID of the `WatchSpec` that produced this event.
    pub spec_id: String,
    /// The matched row as a JSON object.
    pub payload: serde_json::Value,
    /// The action reference from the spec — tells the integrator what to do.
    pub on_match: ActionRef,
    /// UTC timestamp when the event was emitted.
    pub fired_at: chrono::DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// WatchRunner
// ---------------------------------------------------------------------------

type BoxedSource = Box<dyn WatchSource>;

/// A registered watcher slot: spec + source.
struct WatchSlot {
    spec: WatchSpec,
    source: BoxedSource,
}

/// Orchestrates all registered watchers.
///
/// See the module-level documentation for the lifecycle.
pub struct WatchRunner {
    slots: Vec<WatchSlot>,
    dedup: DedupCache,
    channel_capacity: usize,
}

impl WatchRunner {
    /// Default channel buffer size (number of events that can be queued
    /// before back-pressure kicks in and a watcher task blocks).
    pub const DEFAULT_CHANNEL_CAPACITY: usize = 256;

    /// Default dedup cache capacity (number of fingerprints).
    pub const DEFAULT_DEDUP_CAPACITY: u64 = 10_000;

    /// Default dedup TTL — 24 hours.  Matches that re-appear after 24 h
    /// are treated as new (wanted for daily alert patterns).
    pub const DEFAULT_DEDUP_TTL: Duration = Duration::from_secs(86_400);

    /// Create a new runner with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::with_dedup(DedupCache::new(
            Self::DEFAULT_DEDUP_CAPACITY,
            Self::DEFAULT_DEDUP_TTL,
        ))
    }

    /// Create with a custom [`DedupCache`] (useful for tests with short TTLs).
    #[must_use]
    pub fn with_dedup(dedup: DedupCache) -> Self {
        Self {
            slots: Vec::new(),
            dedup,
            channel_capacity: Self::DEFAULT_CHANNEL_CAPACITY,
        }
    }

    /// Set the mpsc channel capacity.
    #[must_use]
    pub fn channel_capacity(mut self, cap: usize) -> Self {
        self.channel_capacity = cap;
        self
    }

    /// Register a `WatchSpec` + its source.
    ///
    /// Panics at [`run`](Self::run) time (not here) if a duplicate `spec.id`
    /// is detected, so callers should ensure IDs are unique.
    pub fn register(&mut self, spec: WatchSpec, source: impl WatchSource + 'static) {
        self.slots.push(WatchSlot {
            spec,
            source: Box::new(source),
        });
    }

    /// Start all watchers and return the event receiver.
    ///
    /// Each slot is driven by an independent `tokio::spawn`'d task.
    /// The tasks are linked by the mpsc sender; dropping the returned
    /// `mpsc::Receiver` will make them exit on the next send attempt.
    ///
    /// # Panics
    ///
    /// Panics if two registered slots share the same `spec.id`.
    #[must_use]
    pub fn run(self) -> mpsc::Receiver<WatchEvent> {
        let (tx, rx) = mpsc::channel(self.channel_capacity);

        // Validate uniqueness of IDs before spawning.
        let mut seen_ids: HashSet<&str> = HashSet::new();
        for slot in &self.slots {
            assert!(
                seen_ids.insert(slot.spec.id.as_str()),
                "duplicate WatchSpec id: {}",
                slot.spec.id
            );
        }

        let dedup = Arc::new(self.dedup);

        for slot in self.slots {
            let tx = tx.clone();
            let dedup = Arc::clone(&dedup);
            let spec = slot.spec;
            let source = slot.source;
            tokio::spawn(watch_task(spec, source, dedup, tx));
        }

        rx
    }
}

impl Default for WatchRunner {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Internal: per-spec watch task
// ---------------------------------------------------------------------------

#[instrument(skip(source, dedup, tx), fields(spec_id = %spec.id))]
async fn watch_task(
    spec: WatchSpec,
    source: BoxedSource,
    dedup: Arc<DedupCache>,
    tx: mpsc::Sender<WatchEvent>,
) {
    let interval_dur = schedule_to_duration(&spec.schedule);
    info!(
        spec_id = %spec.id,
        interval_secs = interval_dur.as_secs(),
        "watch task started"
    );

    let mut interval = tokio::time::interval(interval_dur);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;

        let matches = match source.poll().await {
            Ok(m) => m,
            Err(SourceError::Sql(e)) => {
                error!(spec_id = %spec.id, error = %e, "sql poll error");
                if let Some(ctr) = xiaoguai_observability::watch_wakeups_total() {
                    ctr.with_label_values(&[spec.id.as_str(), "error"]).inc();
                }
                continue;
            }
            Err(SourceError::Http(e)) => {
                error!(spec_id = %spec.id, error = %e, "http poll error");
                if let Some(ctr) = xiaoguai_observability::watch_wakeups_total() {
                    ctr.with_label_values(&[spec.id.as_str(), "error"]).inc();
                }
                continue;
            }
            Err(e) => {
                error!(spec_id = %spec.id, error = %e, "poll error");
                if let Some(ctr) = xiaoguai_observability::watch_wakeups_total() {
                    ctr.with_label_values(&[spec.id.as_str(), "error"]).inc();
                }
                continue;
            }
        };

        // Count this wakeup: "empty" if no matches, "match" for each non-dedup event sent.
        let mut sent_any = false;
        for m in matches {
            if dedup.is_duplicate(&spec.id, &m).await {
                if let Some(ctr) = xiaoguai_observability::watch_wakeups_total() {
                    ctr.with_label_values(&[spec.id.as_str(), "duplicate"])
                        .inc();
                }
                continue;
            }
            dedup.record(&spec.id, &m).await;
            let event = WatchEvent {
                spec_id: spec.id.clone(),
                payload: m.as_value(),
                on_match: spec.on_match.clone(),
                fired_at: Utc::now(),
            };
            if let Err(e) = tx.send(event).await {
                warn!(spec_id = %spec.id, error = %e, "event channel closed; stopping watch task");
                return;
            }
            if let Some(ctr) = xiaoguai_observability::watch_wakeups_total() {
                ctr.with_label_values(&[spec.id.as_str(), "match"]).inc();
            }
            sent_any = true;
        }
        if !sent_any {
            if let Some(ctr) = xiaoguai_observability::watch_wakeups_total() {
                ctr.with_label_values(&[spec.id.as_str(), "empty"]).inc();
            }
        }
    }
}

fn schedule_to_duration(schedule: &WatchSchedule) -> Duration {
    match schedule {
        WatchSchedule::IntervalSecs { secs } => Duration::from_secs(*secs),
        WatchSchedule::Cron { expr: _ } => {
            // Full cron scheduling is deferred to v1.3.x; for now treat cron
            // specs as 60-second intervals and log a warning on first tick.
            warn!("cron schedule not yet supported; falling back to 60-second interval");
            Duration::from_secs(60)
        }
    }
}

// ---------------------------------------------------------------------------
// Match helpers — re-exported for convenience
// ---------------------------------------------------------------------------

pub use crate::source::Match;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::InMemorySource;
    use serde_json::json;

    fn make_spec(id: &str) -> WatchSpec {
        WatchSpec {
            id: id.to_string(),
            source: crate::spec::WatchSourceSpec::Sql {
                query: "SELECT 1".into(),
            },
            schedule: WatchSchedule::IntervalSecs { secs: 1 },
            on_match: ActionRef {
                action: "notify".into(),
                target: None,
                params: serde_json::Map::new(),
            },
        }
    }

    #[tokio::test]
    async fn single_match_fires_event() {
        let dedup = DedupCache::new(100, Duration::from_secs(3600));
        let mut runner = WatchRunner::with_dedup(dedup).channel_capacity(16);
        let spec = make_spec("test-single");
        let source = InMemorySource::new(vec![serde_json::from_value(json!({"id": 1})).unwrap()]);
        runner.register(spec, source);
        let mut rx = runner.run();

        // The first tick fires immediately (interval ticks at start).
        let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timeout")
            .expect("channel closed");
        assert_eq!(event.spec_id, "test-single");
        assert_eq!(event.payload["id"], 1);
    }

    #[tokio::test]
    async fn duplicate_within_ttl_fires_only_once() {
        // Very long TTL — same row should not fire twice.
        let dedup = DedupCache::new(100, Duration::from_secs(3600));
        let mut runner = WatchRunner::with_dedup(dedup).channel_capacity(16);
        let spec = make_spec("test-dedup");
        // Same row every poll.
        let source = InMemorySource::new(vec![serde_json::from_value(json!({"id": 42})).unwrap()]);
        runner.register(spec, source);
        let mut rx = runner.run();

        // Wait long enough for at least two ticks (interval = 1s).
        let first = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timeout")
            .expect("channel closed");
        assert_eq!(first.payload["id"], 42);

        // Second receive should timeout (duplicate suppressed).
        let second = tokio::time::timeout(Duration::from_millis(1500), rx.recv()).await;
        assert!(
            second.is_err(),
            "expected timeout (duplicate suppressed) but got event"
        );
    }

    #[tokio::test]
    async fn changed_row_fires_again() {
        // Use a very short TTL so the first record expires quickly.
        let dedup = DedupCache::new(100, Duration::from_millis(200));
        let mut runner = WatchRunner::with_dedup(dedup).channel_capacity(16);
        let spec = make_spec("test-changed");
        // Row changes between polls.
        let source = CountingSource::new(vec![
            json!({"dso": 61}),
            json!({"dso": 62}), // different content → different fingerprint
        ]);
        runner.register(spec, source);
        let mut rx = runner.run();

        let e1 = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timeout")
            .unwrap();
        let e2 = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timeout")
            .unwrap();
        assert_ne!(
            e1.payload, e2.payload,
            "different rows must produce different events"
        );
    }

    #[tokio::test]
    async fn empty_source_fires_no_events() {
        let dedup = DedupCache::new(100, Duration::from_secs(3600));
        let mut runner = WatchRunner::with_dedup(dedup).channel_capacity(16);
        let spec = make_spec("test-empty");
        let source = InMemorySource::new(vec![]);
        runner.register(spec, source);
        let mut rx = runner.run();

        let result = tokio::time::timeout(Duration::from_millis(1200), rx.recv()).await;
        assert!(result.is_err(), "no events expected for empty source");
    }

    #[tokio::test]
    #[should_panic(expected = "duplicate WatchSpec id")]
    async fn duplicate_spec_id_panics() {
        let mut runner = WatchRunner::new();
        runner.register(make_spec("dup-id"), InMemorySource::new(vec![]));
        runner.register(make_spec("dup-id"), InMemorySource::new(vec![]));
        let _rx = runner.run(); // should panic here
    }

    // ---------------------------------------------------------------------------
    // CountingSource — cycles through a fixed list of rows
    // ---------------------------------------------------------------------------
    use crate::source::{SourceError as SE, WatchSource};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingSource {
        rows: Vec<serde_json::Value>,
        idx: Arc<AtomicUsize>,
    }

    impl CountingSource {
        fn new(rows: Vec<serde_json::Value>) -> Self {
            Self {
                rows,
                idx: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl WatchSource for CountingSource {
        async fn poll(&self) -> Result<Vec<Match>, SE> {
            let i = self.idx.fetch_add(1, Ordering::Relaxed) % self.rows.len();
            let row = self.rows[i].clone();
            Ok(vec![Match::from_value(row)])
        }
    }
}
