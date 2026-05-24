//! File-system watcher source — wraps the `notify-debouncer-full` crate.
//!
//! Each [`FileWatchRoute`] binds one job id to one path. The source
//! registers every path with a single recursive debounced watcher
//! and, on each debounce batch, emits a [`TriggerEvent`] for every
//! route whose `path` is an ancestor of (or equal to) a changed path.
//!
//! ## Debounce window
//!
//! The debounce window defaults to **250 ms** and is configurable via
//! the `FILE_WATCH_DEBOUNCE_MS` environment variable (integer
//! milliseconds).  Bursty operations such as an editor save sequence
//! (`rename tmp → target`, `write`, `chmod`) that arrive within the
//! window are coalesced into one batch, so the scheduler receives one
//! `TriggerEvent` per logical change instead of three.
//!
//! ## Threading model
//!
//! `notify-debouncer-full` runs two background threads:
//!
//! 1. The OS-native watcher thread (inotify / kqueue / `FSEvents`).
//! 2. A debouncer tick thread that flushes the coalesced batch after
//!    the window expires and calls our handler with
//!    `DebounceEventResult`.
//!
//! We bridge from the handler callback (called on the debouncer tick
//! thread) into the Tokio event channel via a
//! `std::sync::mpsc::channel`.  A `spawn_blocking` task drains that
//! channel and forwards `TriggerEvent`s to the runner.
//!
//! Dropping the `Debouncer` guard stops both background threads.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
// notify-debouncer-full re-exports the underlying notify crate, so we
// import notify through that re-export to avoid an extra direct dep.
use notify_debouncer_full::notify::{self, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};
use parking_lot::Mutex;

use crate::trigger_source::{EventSender, SourceError, TriggerEvent, TriggerSource};

/// Default debounce window when `FILE_WATCH_DEBOUNCE_MS` is not set.
pub const DEFAULT_DEBOUNCE_MS: u64 = 250;

/// Read the debounce window from the environment, falling back to
/// [`DEFAULT_DEBOUNCE_MS`].
fn debounce_duration() -> Duration {
    let ms = std::env::var("FILE_WATCH_DEBOUNCE_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_DEBOUNCE_MS);
    Duration::from_millis(ms)
}

/// One (`job_id`, `path`) binding. The source emits a
/// [`TriggerEvent`] for the bound `job_id` whenever a debounced
/// batch touches `path` (or anything under it, recursively).
#[derive(Debug, Clone)]
pub struct FileWatchRoute {
    pub job_id: String,
    pub path: PathBuf,
}

impl FileWatchRoute {
    #[must_use]
    pub fn new(job_id: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            job_id: job_id.into(),
            path: path.into(),
        }
    }
}

// Type alias to keep field declarations readable.
type ArcDebouncer = Debouncer<notify::RecommendedWatcher, RecommendedCache>;

/// Filesystem-watch source backed by `notify-debouncer-full`.
///
/// Construct with [`FileWatchSource::new`], register routes via
/// [`FileWatchSource::add_route`], then hand it to the runner via
/// [`TriggerSource::start`].
///
/// The source owns the [`Debouncer`] guard internally; dropping the
/// source stops the background watcher and debouncer threads.
pub struct FileWatchSource {
    routes: Arc<Mutex<Vec<FileWatchRoute>>>,
    debouncer: Mutex<Option<ArcDebouncer>>,
}

impl FileWatchSource {
    #[must_use]
    pub fn new() -> Self {
        Self {
            routes: Arc::new(Mutex::new(Vec::new())),
            debouncer: Mutex::new(None),
        }
    }

    /// Register a (`job_id`, `path`) binding.
    ///
    /// May be called before or after [`TriggerSource::start`] — when
    /// called after, the new path is added to the live debouncer.
    pub fn add_route(&self, route: FileWatchRoute) -> Result<(), SourceError> {
        if let Some(d) = self.debouncer.lock().as_mut() {
            d.watch(&route.path, RecursiveMode::Recursive)
                .map_err(|e| SourceError::Backend(e.to_string()))?;
        }
        self.routes.lock().push(route);
        Ok(())
    }

    /// True iff a notify `EventKind` should propagate to the runner.
    ///
    /// We accept create / modify / remove events; pure access events
    /// (Linux `inotify` `IN_ACCESS`) are ignored because they fire on
    /// every read.
    fn should_fire(kind: notify::EventKind) -> bool {
        use notify::event::{ModifyKind, RemoveKind};
        matches!(
            kind,
            notify::EventKind::Create(_)
                | notify::EventKind::Remove(RemoveKind::File | RemoveKind::Folder)
                | notify::EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Name(_))
        )
    }

    /// True iff `route.path` is `changed` or an ancestor of `changed`.
    fn route_matches(route_path: &Path, changed: &Path) -> bool {
        if route_path == changed {
            return true;
        }
        changed.starts_with(route_path)
    }
}

impl Default for FileWatchSource {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for FileWatchSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileWatchSource")
            .field("route_count", &self.routes.lock().len())
            .field("started", &self.debouncer.lock().is_some())
            .finish()
    }
}

#[async_trait]
impl TriggerSource for FileWatchSource {
    fn id(&self) -> &'static str {
        "file_watch"
    }

    async fn start(&self, tx: EventSender) -> Result<(), SourceError> {
        if self.debouncer.lock().is_some() {
            return Err(SourceError::AlreadyStarted);
        }

        let timeout = debounce_duration();

        // The debouncer handler is called on a background thread managed
        // by notify-debouncer-full.  We forward batches through a sync
        // channel so the blocking drain loop can live in spawn_blocking.
        let (batch_tx, batch_rx) = std::sync::mpsc::channel::<DebounceEventResult>();

        let debouncer = new_debouncer(timeout, None, move |res: DebounceEventResult| {
            // best-effort — if the receiver is gone we're shutting down.
            let _ = batch_tx.send(res);
        })
        .map_err(|e| SourceError::Backend(e.to_string()))?;

        // Stash the debouncer so it stays alive (drop = stop).
        *self.debouncer.lock() = Some(debouncer);

        // Watch every already-registered route.
        let initial = self.routes.lock().clone();
        for r in &initial {
            if let Some(d) = self.debouncer.lock().as_mut() {
                d.watch(&r.path, RecursiveMode::Recursive)
                    .map_err(|e| SourceError::Backend(e.to_string()))?;
            }
        }

        let routes = self.routes.clone();
        // The drain loop blocks on the sync channel; spawn_blocking is
        // the right home so we don't block the Tokio worker pool.
        tokio::task::spawn_blocking(move || {
            while let Ok(res) = batch_rx.recv() {
                let events = match res {
                    Ok(evs) => evs,
                    Err(errs) => {
                        for e in errs {
                            tracing::warn!(error = %e, "notify-debouncer error");
                        }
                        continue;
                    }
                };

                // One debounced batch may cover multiple changed paths.
                // Iterate paths × routes and emit one TriggerEvent per match.
                let snapshot = routes.lock().clone();
                for debounced in &events {
                    // DebouncedEvent derefs to notify::Event.
                    if !Self::should_fire(debounced.kind) {
                        continue;
                    }
                    for path in &debounced.paths {
                        for route in &snapshot {
                            if !Self::route_matches(&route.path, path) {
                                continue;
                            }
                            let ev = TriggerEvent::new(route.job_id.clone()).with_detail(
                                serde_json::json!({
                                    "source": "file_watch",
                                    "changed_path": path.display().to_string(),
                                    "kind": format!("{:?}", debounced.kind),
                                }),
                            );
                            // blocking_send is correct: we're in spawn_blocking.
                            if tx.blocking_send(ev).is_err() {
                                // Receiver dropped — runner shut down.
                                return;
                            }
                        }
                    }
                }
            }
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_matches_self_and_descendants() {
        let root = Path::new("/tmp/watch");
        assert!(FileWatchSource::route_matches(root, root));
        assert!(FileWatchSource::route_matches(
            root,
            Path::new("/tmp/watch/a.md")
        ));
        assert!(FileWatchSource::route_matches(
            root,
            Path::new("/tmp/watch/sub/b.md")
        ));
    }

    #[test]
    fn route_does_not_match_sibling() {
        let root = Path::new("/tmp/watch");
        assert!(!FileWatchSource::route_matches(
            root,
            Path::new("/tmp/other.md")
        ));
    }

    #[test]
    fn should_fire_filters_pure_access() {
        use notify::event::{AccessKind, AccessMode};
        let k = notify::EventKind::Access(AccessKind::Read);
        assert!(!FileWatchSource::should_fire(k));
        let k = notify::EventKind::Access(AccessKind::Open(AccessMode::Read));
        assert!(!FileWatchSource::should_fire(k));
    }

    #[test]
    fn default_debounce_ms_constant_is_250() {
        // Tests the exported constant directly — no env-var manipulation,
        // safe to run in parallel with other tests.
        assert_eq!(DEFAULT_DEBOUNCE_MS, 250);
        assert_eq!(
            Duration::from_millis(DEFAULT_DEBOUNCE_MS),
            Duration::from_millis(250)
        );
    }
}
