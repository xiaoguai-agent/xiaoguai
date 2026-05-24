//! End-to-end tests for `PgJobRepository` + `PgJobRunRepository` against
//! a real Postgres in a container. All tests `#[ignore]` so CI's fast
//! path stays Docker-free (consistent with `xiaoguai-storage`'s own
//! testcontainer suites — see those for prior art).
//!
//! Run with `cargo test -p xiaoguai-scheduler --test pg_repository_e2e -- --ignored`.

use std::sync::Arc;

use chrono::Utc;
use sqlx::PgPool;
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{runners::AsyncRunner, ContainerAsync},
};
use xiaoguai_scheduler::{
    JobRepository, JobRun, JobRunRepository, JobRunStatus, PgJobRepository, PgJobRunRepository,
    ScheduledJob, Trigger,
};
use xiaoguai_storage::db;

async fn setup() -> (PgPool, ContainerAsync<Postgres>) {
    let pg = Postgres::default().start().await.expect("start pg");
    let port = pg.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = db::connect(&url, 5).await.expect("connect");
    db::migrate(&pool).await.expect("migrate");
    (pool, pg)
}

async fn insert_tenant(pool: &PgPool, id: &str) {
    sqlx::query(
        "INSERT INTO tenants (id, name, display_name, status, created_at)
         VALUES ($1, $1, $1, 'active', NOW())
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(id)
    .execute(pool)
    .await
    .expect("insert tenant");
}

fn sample_job(id: &str, tenant: Option<String>) -> ScheduledJob {
    ScheduledJob::new(
        id,
        tenant,
        format!("job-{id}"),
        Trigger::interval(60).unwrap(),
        serde_json::json!({"prompt": "scan"}),
    )
}

fn sample_run(job: &ScheduledJob) -> JobRun {
    JobRun {
        id: 0,
        job_id: job.id.clone(),
        tenant_id: job.tenant_id.clone(),
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
#[ignore = "requires Docker"]
async fn upsert_then_get_round_trips_job() {
    let (pool, _pg) = setup().await;
    insert_tenant(&pool, "tenant-x").await;
    let repo = Arc::new(PgJobRepository::new(pool.clone()));

    let job = sample_job("j1", Some("tenant-x".into()));
    repo.upsert(&job).await.unwrap();

    let back = repo.get("j1").await.unwrap();
    assert_eq!(back.id, "j1");
    assert_eq!(back.tenant_id.as_deref(), Some("tenant-x"));
    assert!(back.enabled);
    assert!(back.trigger.is_scheduled());
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn list_due_returns_scheduled_jobs_with_no_next_fire() {
    let (pool, _pg) = setup().await;
    insert_tenant(&pool, "tenant-x").await;
    let repo = Arc::new(PgJobRepository::new(pool.clone()));

    repo.upsert(&sample_job("a", Some("tenant-x".into())))
        .await
        .unwrap();
    repo.upsert(&sample_job("b", Some("tenant-x".into())))
        .await
        .unwrap();

    let due = repo.list_due(Utc::now(), 10).await.unwrap();
    let ids: Vec<_> = due.iter().map(|j| j.id.clone()).collect();
    assert!(ids.contains(&"a".to_string()));
    assert!(ids.contains(&"b".to_string()));
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn list_due_filters_reactive_jobs() {
    let (pool, _pg) = setup().await;
    insert_tenant(&pool, "tenant-x").await;
    let repo = Arc::new(PgJobRepository::new(pool.clone()));

    let mut reactive = sample_job("react", Some("tenant-x".into()));
    reactive.trigger = Trigger::file_watch("/tmp/watch").unwrap();
    repo.upsert(&reactive).await.unwrap();
    repo.upsert(&sample_job("scheduled", Some("tenant-x".into())))
        .await
        .unwrap();

    let due = repo.list_due(Utc::now(), 10).await.unwrap();
    let ids: Vec<_> = due.iter().map(|j| j.id.clone()).collect();
    assert!(ids.contains(&"scheduled".to_string()));
    assert!(!ids.contains(&"react".to_string()));
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn record_fire_updates_bookkeeping() {
    let (pool, _pg) = setup().await;
    insert_tenant(&pool, "tenant-x").await;
    let repo = Arc::new(PgJobRepository::new(pool.clone()));

    repo.upsert(&sample_job("j1", Some("tenant-x".into())))
        .await
        .unwrap();
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
#[ignore = "requires Docker"]
async fn run_repo_insert_assigns_id_and_update_status_round_trips() {
    let (pool, _pg) = setup().await;
    insert_tenant(&pool, "tenant-x").await;
    let jobs = Arc::new(PgJobRepository::new(pool.clone()));
    let runs = Arc::new(PgJobRunRepository::new(pool.clone()));

    let job = sample_job("j1", Some("tenant-x".into()));
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
#[ignore = "requires Docker"]
async fn record_fire_on_missing_job_errors() {
    let (pool, _pg) = setup().await;
    let repo = PgJobRepository::new(pool.clone());
    let err = repo
        .record_fire("nope", Utc::now(), None)
        .await
        .unwrap_err();
    assert!(format!("{err}").contains("nope"));
}
