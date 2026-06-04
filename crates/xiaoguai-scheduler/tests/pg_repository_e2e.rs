//! End-to-end tests for `PgJobRepository` + `PgJobRunRepository` against a
//! temp `SQLite` database (DEC-033 single-user pivot). The repos are now
//! `SQLite`-backed; these run on every `cargo test` (no Docker, no `#[ignore]`).

use std::sync::Arc;

use chrono::Utc;
use sqlx::SqlitePool;
use tempfile::TempDir;
use xiaoguai_scheduler::{
    JobRepository, JobRun, JobRunRepository, JobRunStatus, PgJobRepository, PgJobRunRepository,
    ScheduledJob, Trigger,
};
use xiaoguai_storage::db;

/// Returns a connected+migrated temp `SQLite` pool. The returned `TempDir` must
/// stay alive for the duration of the test (dropping it deletes the DB file).
async fn setup() -> (SqlitePool, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("t.db");
    let pool = db::connect(path.to_str().expect("utf8 path"), 5)
        .await
        .expect("connect");
    db::migrate(&pool).await.expect("migrate");
    (pool, dir)
}

fn sample_job(id: &str) -> ScheduledJob {
    ScheduledJob::new(
        id,
        format!("job-{id}"),
        Trigger::interval(60).unwrap(),
        serde_json::json!({"prompt": "scan"}),
    )
}

fn sample_run(job: &ScheduledJob) -> JobRun {
    JobRun {
        id: 0,
        job_id: job.id.clone(),
        status: JobRunStatus::Running,
        attempt: 1,
        started_at: Some(Utc::now()),
        finished_at: None,
        session_id: None,
        error_message: None,
        output_preview: None,
        created_at: Utc::now(),
    }
}

#[tokio::test]
async fn upsert_then_get_round_trips_job() {
    let (pool, _dir) = setup().await;
    let repo = Arc::new(PgJobRepository::new(pool.clone()));

    let job = sample_job("j1");
    repo.upsert(&job).await.unwrap();

    let back = repo.get("j1").await.unwrap();
    assert_eq!(back.id, "j1");
    assert!(back.enabled);
    assert!(back.trigger.is_scheduled());
}

#[tokio::test]
async fn list_due_returns_scheduled_jobs_with_no_next_fire() {
    let (pool, _dir) = setup().await;
    let repo = Arc::new(PgJobRepository::new(pool.clone()));

    repo.upsert(&sample_job("a")).await.unwrap();
    repo.upsert(&sample_job("b")).await.unwrap();

    let due = repo.list_due(Utc::now(), 10).await.unwrap();
    let ids: Vec<_> = due.iter().map(|j| j.id.clone()).collect();
    assert!(ids.contains(&"a".to_string()));
    assert!(ids.contains(&"b".to_string()));
}

#[tokio::test]
async fn list_due_filters_reactive_jobs() {
    let (pool, _dir) = setup().await;
    let repo = Arc::new(PgJobRepository::new(pool.clone()));

    let mut reactive = sample_job("react");
    reactive.trigger = Trigger::file_watch("/tmp/watch").unwrap();
    repo.upsert(&reactive).await.unwrap();
    repo.upsert(&sample_job("scheduled")).await.unwrap();

    let due = repo.list_due(Utc::now(), 10).await.unwrap();
    let ids: Vec<_> = due.iter().map(|j| j.id.clone()).collect();
    assert!(ids.contains(&"scheduled".to_string()));
    assert!(!ids.contains(&"react".to_string()));
}

#[tokio::test]
async fn list_reactive_returns_only_reactive_enabled_jobs() {
    let (pool, _dir) = setup().await;
    let repo = Arc::new(PgJobRepository::new(pool.clone()));

    let mut watch = sample_job("watch-1");
    watch.trigger = Trigger::file_watch("/var/notes").unwrap();
    repo.upsert(&watch).await.unwrap();

    let mut disabled = sample_job("watch-disabled");
    disabled.trigger = Trigger::file_watch("/var/disabled").unwrap();
    disabled.enabled = false;
    repo.upsert(&disabled).await.unwrap();

    repo.upsert(&sample_job("scheduled")).await.unwrap();

    let got = repo.list_reactive().await.unwrap();
    let ids: Vec<_> = got.iter().map(|j| j.id.clone()).collect();
    assert!(ids.contains(&"watch-1".to_string()));
    assert!(!ids.contains(&"scheduled".to_string()));
    assert!(!ids.contains(&"watch-disabled".to_string()));
}

#[tokio::test]
async fn record_fire_updates_bookkeeping() {
    let (pool, _dir) = setup().await;
    let repo = Arc::new(PgJobRepository::new(pool.clone()));

    repo.upsert(&sample_job("j1")).await.unwrap();
    let now = Utc::now();
    let next = now + chrono::Duration::seconds(60);
    repo.record_fire("j1", now, Some(next)).await.unwrap();

    let back = repo.get("j1").await.unwrap();
    assert!(back.last_fire_at.is_some());
    assert_eq!(
        back.next_fire_at.map(|t| t.timestamp()),
        Some(next.timestamp())
    );
}

#[tokio::test]
async fn run_repo_insert_assigns_id_and_update_status_round_trips() {
    let (pool, _dir) = setup().await;
    let jobs = Arc::new(PgJobRepository::new(pool.clone()));
    let runs = Arc::new(PgJobRunRepository::new(pool.clone()));

    let job = sample_job("j1");
    jobs.upsert(&job).await.unwrap();

    let a = runs.insert(sample_run(&job)).await.unwrap();
    let b = runs.insert(sample_run(&job)).await.unwrap();
    assert!(a.id < b.id, "ids should be monotonic");

    runs.update_status(
        a.id,
        JobRunStatus::Succeeded,
        Some(Utc::now()),
        None,
        Some("hello".into()),
        None,
    )
    .await
    .unwrap();

    let list = runs.list_for_job("j1", 10).await.unwrap();
    assert_eq!(list.len(), 2);
    // list_for_job orders by id DESC, so b is first.
    assert_eq!(list[0].id, b.id);
    let updated = list.iter().find(|r| r.id == a.id).unwrap();
    assert_eq!(updated.status, JobRunStatus::Succeeded);
    assert_eq!(updated.output_preview.as_deref(), Some("hello"));
}

#[tokio::test]
async fn record_fire_on_missing_job_errors() {
    let (pool, _dir) = setup().await;
    let repo = PgJobRepository::new(pool.clone());
    let err = repo
        .record_fire("nope", Utc::now(), None)
        .await
        .unwrap_err();
    assert!(format!("{err}").contains("nope"));
}
