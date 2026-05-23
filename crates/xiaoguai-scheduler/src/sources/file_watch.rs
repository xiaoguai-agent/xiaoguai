//! File-system watcher source — wraps the `notify` crate.
//!
//! Each [`FileWatchRoute`] binds one job id to one path. The source
//! registers every path with a single recursive recommended watcher
//! and, on each event, emits a [`TriggerEvent`] for every route whose
//! `path` is an ancestor of (or equal to) the changed path.
//!
//! Debounce is intentionally minimal: `notify`'s recommended watcher
//! already coalesces back-end events into one batch per syscall. A
//! single `cp file dst` therefore yields one event per file written,
//! not one per byte. If user feedback shows we need stronger
//! debouncing we'll layer `notify-debouncer-full` in v0.10.1.x.
//!
//! The watcher runs on `notify`'s own dedicated thread; this module
//! only owns the small Tokio-side glue (a `std::sync::mpsc` ↔
//! `tokio::sync::mpsc` bridge task) so the runner stays single-mpsc.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use notify::{
    event::{ModifyKind, RemoveKind},
    Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use parking_lot::Mutex;

use crate::trigger_source::{EventSender, SourceError, TriggerEvent, TriggerSource};

/// One (`job_id`, `path`) binding. The source emits a
/// [`TriggerEvent`] for the bound `job_id` whenever a `notify::Event`
/// touches `path` (or anything under it, recursively).
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

/// Filesystem-watch source. Construct with [`FileWatchSource::new`],
/// register routes via [`FileWatchSource::add_route`], then hand it
/// to the runner via [`TriggerSource::start`].
///
/// The source holds the [`RecommendedWatcher`] internally so dropping
/// the source stops the background thread.
pub struct FileWatchSource {
    routes: Arc<Mutex<Vec<FileWatchRoute>>>,
    watcher: Mutex<Option<RecommendedWatcher>>,
}

impl FileWatchSource {
    #[must_use]
    pub fn new() -> Self {
        Self {
            routes: Arc::new(Mutex::new(Vec::new())),
            watcher: Mutex::new(None),
        }
    }

    /// Register a (`job_id`, `path`) binding. May be called before or
    /// after [`TriggerSource::start`] — when called after, the new
    /// path is added to the existing watcher.
    pub fn add_route(&self, route: FileWatchRoute) -> Result<(), SourceError> {
        if let Some(w) = self.watcher.lock().as_mut() {
            w.watch(&route.path, RecursiveMode::Recursive)
                .map_err(|e| SourceError::Backend(e.to_string()))?;
        }
        self.routes.lock().push(route);
        Ok(())
    }

    /// True iff a notify event should fire a job.
    ///
    /// We accept create/modify/remove events; pure access events
    /// (Linux `inotify` `IN_ACCESS`) are ignored because they fire on
    /// every read and would saturate the runner.
    fn should_fire(kind: EventKind) -> bool {
        matches!(
            kind,
            EventKind::Create(_)
                | EventKind::Remove(RemoveKind::File | RemoveKind::Folder)
                | EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Name(_))
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
            .field("started", &self.watcher.lock().is_some())
            .finish()
    }
}

#[async_trait]
impl TriggerSource for FileWatchSource {
    fn id(&self) -> &'static str {
        "file_watch"
    }

    async fn start(&self, tx: EventSender) -> Result<(), SourceError> {
        if self.watcher.lock().is_some() {
            return Err(SourceError::AlreadyStarted);
        }

        // notify hands events to a std-thread callback; bounce them
        // through a sync channel and drain into the tokio channel from
        // a small background task.
        let (raw_tx, raw_rx) = std::sync::mpsc::channel::<notify::Result<Event>>();
        let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res| {
            // best-effort — receiver gone means we're shutting down.
            let _ = raw_tx.send(res);
        })
        .map_err(|e| SourceError::Backend(e.to_string()))?;

        // Watch every already-registered route.
        let initial = self.routes.lock().clone();
        for r in initial {
            watcher
                .watch(&r.path, RecursiveMode::Recursive)
                .map_err(|e| SourceError::Backend(e.to_string()))?;
        }

        // Stash the watcher so it stays alive (drop = stop).
        *self.watcher.lock() = Some(watcher);

        let routes = self.routes.clone();
        // The drain loop blocks on the std channel; tokio::task::spawn_blocking
        // is the right home for it.
        tokio::task::spawn_blocking(move || {
            while let Ok(res) = raw_rx.recv() {
                let event = match res {
                    Ok(ev) => ev,
                    Err(e) => {
                        tracing::warn!(error = %e, "notify error");
                        continue;
                    }
                };
                if !Self::should_fire(event.kind) {
                    continue;
                }
                let snapshot = routes.lock().clone();
                for path in &event.paths {
                    for route in &snapshot {
                        if !Self::route_matches(&route.path, path) {
                            continue;
                        }
                        let ev = TriggerEvent::new(route.job_id.clone()).with_detail(
                            serde_json::json!({
                                "source": "file_watch",
                                "changed_path": path.display().to_string(),
                                "kind": format!("{:?}", event.kind),
                            }),
                        );
                        // blocking_send is correct here: this task is
                        // spawn_blocking, not a tokio runtime worker.
                        if tx.blocking_send(ev).is_err() {
                            // Receiver dropped — runner shut down. Bail.
                            return;
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
        // We can't construct every notify EventKind ergonomically
        // (the inner shape varies by backend), but we can at least
        // assert that Access events don't fire.
        use notify::event::{AccessKind, AccessMode};
        let k = EventKind::Access(AccessKind::Read);
        assert!(!FileWatchSource::should_fire(k));
        let k = EventKind::Access(AccessKind::Open(AccessMode::Read));
        assert!(!FileWatchSource::should_fire(k));
    }
}
