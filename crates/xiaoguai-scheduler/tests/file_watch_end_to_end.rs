//! End-to-end test for [`FileWatchSource`] — touch a real file under
//! a real `notify` watcher and verify the runner picks it up.
//!
//! Lives in `tests/` rather than as a `#[cfg(test)] mod` inside the
//! crate because:
//!
//! * It needs `tokio::test(flavor = "multi_thread")` (the
//!   `notify` recommended watcher uses a background thread and
//!   `spawn_blocking`).
//! * It exercises the full external surface (start → fs event →
//!   event channel → `fire_event` → audit/runs rows) — that's an
//!   integration test by definition.
//!
//! Timing budget is generous (1.5 s) because the macOS fsevent
//! backend coalesces events with a small kernel delay; the test
//! polls instead of sleeping a fixed amount.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tempfile::TempDir;
use xiaoguai_scheduler::{
    event_channel, EchoExecutor, FileWatchRoute, FileWatchSource, InMemoryJobRepository,
    InMemoryJobRunRepository, JobRepository, JobRunner, RecordingAuditAppender, ScheduledJob,
    Trigger, TriggerSource,
};

fn job_for_path(id: &str, path: &Path) -> ScheduledJob {
    ScheduledJob::new(
        id,
        id,
        Trigger::file_watch(path.display().to_string()).unwrap(),
        serde_json::json!({"prompt": "scan"}),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_change_fires_matching_job() {
    let dir = TempDir::new().unwrap();
    // Canonicalize — on macOS TempDir hands back /var/... while
    // fsevent reports paths as /private/var/..., breaking the
    // ancestor-match unless we resolve both sides through the same
    // realpath.
    let root: PathBuf = std::fs::canonicalize(dir.path()).unwrap();

    let jobs: Arc<InMemoryJobRepository> = Arc::new(InMemoryJobRepository::new());
    let runs: Arc<InMemoryJobRunRepository> = Arc::new(InMemoryJobRunRepository::new());
    let audit: Arc<RecordingAuditAppender> = Arc::new(RecordingAuditAppender::new());
    let runner = JobRunner::new(jobs.clone(), runs.clone(), Arc::new(EchoExecutor), audit);

    jobs.upsert(&job_for_path("j1", &root)).await.unwrap();

    let source = Arc::new(FileWatchSource::new());
    source
        .add_route(FileWatchRoute::new("j1", root.clone()))
        .unwrap();

    let (tx, rx) = event_channel();
    source.start(tx).await.unwrap();

    let runner = Arc::new(runner);
    let runner_for_task = runner.clone();
    let loop_handle = tokio::spawn(async move {
        // No timer — we're only proving the event arm.
        runner_for_task.run_loop(rx, None).await.unwrap();
    });

    // Let the watcher fully register before we touch anything;
    // fsevent in particular needs a beat to start streaming events.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Touch the directory a few times — fsevent on macOS is bursty,
    // a single write is sometimes coalesced away if it lands in the
    // exact moment the backend is settling.
    for i in 0..5 {
        let target = root.join(format!("note-{i}.md"));
        tokio::fs::write(&target, b"hello").await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        if !runs.snapshot().is_empty() {
            break;
        }
    }

    // Poll for the run to land. Budget: 4s total, poll every 50ms.
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if !runs.snapshot().is_empty() {
            break;
        }
        if Instant::now() > deadline {
            drop(source);
            loop_handle.abort();
            panic!("FileWatchSource didn't fire within 4s");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Drop the source → notify thread exits → tx side closes →
    // run_loop returns.
    drop(source);
    let _ = tokio::time::timeout(Duration::from_millis(500), loop_handle).await;

    let runs_snap = runs.snapshot();
    assert!(!runs_snap.is_empty(), "at least one run should have fired");
    assert!(runs_snap.iter().any(|r| r.job_id == "j1"));
}
