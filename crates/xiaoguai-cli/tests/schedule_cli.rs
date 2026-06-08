//! Unit tests for the `schedule` subcommand business logic.
//!
//! These exercise the command functions directly with the scheduler crate's
//! in-memory repositories + `RecordingAuditAppender` — fast, no DB. The
//! `SQLite` path is covered by `xiaoguai-scheduler/tests/sqlite_repository_e2e.rs`.

use chrono::Utc;
use xiaoguai_cli::commands::schedule::{
    create, delete, format_detail, format_table, list, resolve, run_now, set_enabled, show,
    CreateArgs,
};
use xiaoguai_scheduler::{
    InMemoryJobRepository, InMemoryJobRunRepository, JobRepository, JobRun, JobRunRepository,
    JobRunStatus, RecordingAuditAppender, ScheduledJob, Trigger,
};

fn create_args(name: &str) -> CreateArgs {
    CreateArgs {
        name: name.into(),
        cron: "0 0 8 * * *".into(),
        prompt: "scan the inbox".into(),
        description: None,
        sinks: vec![],
    }
}

fn finished_run(job_id: &str, status: JobRunStatus) -> JobRun {
    JobRun {
        id: 0,
        job_id: job_id.into(),
        status,
        attempt: 1,
        started_at: Some(Utc::now()),
        finished_at: Some(Utc::now()),
        session_id: None,
        error_message: None,
        output_preview: None,
        created_at: Utc::now(),
    }
}

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_persists_job_with_prompt_payload_and_next_fire() {
    let repo = InMemoryJobRepository::new();
    let audit = RecordingAuditAppender::new();

    let job = create(&repo, &audit, create_args("daily-scan"))
        .await
        .expect("create ok");

    assert!(job.id.starts_with("job_"), "id is namespaced: {}", job.id);
    assert_eq!(job.name, "daily-scan");
    assert_eq!(job.payload["prompt"], "scan the inbox");
    assert!(job.enabled);
    assert!(
        job.next_fire_at.is_some(),
        "next fire is primed eagerly so a new job doesn't fire immediately"
    );

    let back = repo.get(&job.id).await.expect("persisted");
    assert_eq!(back, job);
}

#[tokio::test]
async fn create_rejects_invalid_cron_with_teaching_error() {
    let repo = InMemoryJobRepository::new();
    let audit = RecordingAuditAppender::new();

    let mut args = create_args("bad");
    args.cron = "not a cron".into();
    let err = create(&repo, &audit, args).await.unwrap_err().to_string();

    assert!(err.contains("not a cron"), "echoes the input: {err}");
    assert!(err.contains("6-field"), "teaches the format: {err}");
    assert!(err.contains("0 0 8 * * *"), "gives an example: {err}");
}

#[tokio::test]
async fn create_rejects_empty_name_and_prompt() {
    let repo = InMemoryJobRepository::new();
    let audit = RecordingAuditAppender::new();

    let mut no_name = create_args("  ");
    no_name.name = "  ".into();
    let err = create(&repo, &audit, no_name)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("--name"), "{err}");

    let mut no_prompt = create_args("ok");
    no_prompt.prompt = String::new();
    let err = create(&repo, &audit, no_prompt)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("--prompt"), "{err}");
}

#[tokio::test]
async fn create_carries_description_and_sinks() {
    let repo = InMemoryJobRepository::new();
    let audit = RecordingAuditAppender::new();

    let mut args = create_args("with-extras");
    args.description = Some("morning digest".into());
    args.sinks = vec!["inbox:owner".into(), "feishu:chat-x".into()];
    let job = create(&repo, &audit, args).await.expect("create ok");

    assert_eq!(job.description.as_deref(), Some("morning digest"));
    assert_eq!(job.sinks, vec!["inbox:owner", "feishu:chat-x"]);
}

// ---------------------------------------------------------------------------
// list + show
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_includes_last_run_status_and_respects_limit() {
    let repo = InMemoryJobRepository::new();
    let runs = InMemoryJobRunRepository::new();
    let audit = RecordingAuditAppender::new();

    let a = create(&repo, &audit, create_args("job-a")).await.unwrap();
    let _b = create(&repo, &audit, create_args("job-b")).await.unwrap();
    runs.insert(finished_run(&a.id, JobRunStatus::Failed))
        .await
        .unwrap();
    runs.insert(finished_run(&a.id, JobRunStatus::Succeeded))
        .await
        .unwrap();

    let rows = list(&repo, &runs, 50).await.expect("list ok");
    assert_eq!(rows.len(), 2);
    let row_a = rows.iter().find(|r| r.job.id == a.id).unwrap();
    // Latest run wins.
    assert_eq!(
        row_a.last_run.as_ref().map(|r| r.status),
        Some(JobRunStatus::Succeeded)
    );

    let capped = list(&repo, &runs, 1).await.expect("list ok");
    assert_eq!(capped.len(), 1);
}

#[tokio::test]
async fn show_returns_job_with_recent_runs() {
    let repo = InMemoryJobRepository::new();
    let runs = InMemoryJobRunRepository::new();
    let audit = RecordingAuditAppender::new();

    let job = create(&repo, &audit, create_args("detail")).await.unwrap();
    runs.insert(finished_run(&job.id, JobRunStatus::Succeeded))
        .await
        .unwrap();

    let (got, history) = show(&repo, &runs, &job.id).await.expect("show ok");
    assert_eq!(got.id, job.id);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, JobRunStatus::Succeeded);
}

// ---------------------------------------------------------------------------
// id resolution (short ids from the list table)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resolve_accepts_unique_prefix_and_rejects_ambiguous_or_unknown() {
    let repo = InMemoryJobRepository::new();
    repo.upsert(&ScheduledJob::new(
        "job_aaaa1111",
        "one",
        Trigger::interval(60).unwrap(),
        serde_json::json!({"prompt": "x"}),
    ))
    .await
    .unwrap();
    repo.upsert(&ScheduledJob::new(
        "job_aabb2222",
        "two",
        Trigger::interval(60).unwrap(),
        serde_json::json!({"prompt": "x"}),
    ))
    .await
    .unwrap();

    // Exact id.
    let exact = resolve(&repo, "job_aaaa1111").await.expect("exact");
    assert_eq!(exact.name, "one");

    // Unique prefix.
    let by_prefix = resolve(&repo, "job_aaaa").await.expect("prefix");
    assert_eq!(by_prefix.id, "job_aaaa1111");

    // Ambiguous prefix → teaching error listing candidates.
    let err = resolve(&repo, "job_aa").await.unwrap_err().to_string();
    assert!(err.contains("ambiguous"), "{err}");
    assert!(
        err.contains("job_aaaa1111") && err.contains("job_aabb2222"),
        "{err}"
    );

    // Unknown id → points at `schedule list`.
    let err = resolve(&repo, "job_zz").await.unwrap_err().to_string();
    assert!(err.contains("schedule list"), "{err}");
}

// ---------------------------------------------------------------------------
// pause / resume / delete
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pause_then_resume_round_trip() {
    let repo = InMemoryJobRepository::new();
    let audit = RecordingAuditAppender::new();
    let job = create(&repo, &audit, create_args("toggler")).await.unwrap();

    let paused = set_enabled(&repo, &audit, &job.id, false).await.unwrap();
    assert!(!paused.enabled);
    assert!(!repo.get(&job.id).await.unwrap().enabled);

    let resumed = set_enabled(&repo, &audit, &job.id, true).await.unwrap();
    assert!(resumed.enabled);
    assert!(
        resumed.next_fire_at.is_some(),
        "resume recomputes the next fire from now (no stale immediate fire)"
    );
}

#[tokio::test]
async fn delete_removes_job_and_second_delete_errors() {
    let repo = InMemoryJobRepository::new();
    let runs = InMemoryJobRunRepository::new();
    let audit = RecordingAuditAppender::new();
    let job = create(&repo, &audit, create_args("doomed")).await.unwrap();

    let gone = delete(&repo, &audit, &job.id).await.expect("delete ok");
    assert_eq!(gone.id, job.id);
    assert!(list(&repo, &runs, 50).await.unwrap().is_empty());

    let err = delete(&repo, &audit, &job.id)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("schedule list"), "{err}");
}

// ---------------------------------------------------------------------------
// audit linkage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn write_ops_append_audit_entries() {
    let repo = InMemoryJobRepository::new();
    let audit = RecordingAuditAppender::new();

    let job = create(&repo, &audit, create_args("audited")).await.unwrap();
    set_enabled(&repo, &audit, &job.id, false).await.unwrap();
    set_enabled(&repo, &audit, &job.id, true).await.unwrap();
    delete(&repo, &audit, &job.id).await.unwrap();

    let entries = audit.snapshot();
    let actions: Vec<_> = entries.iter().map(|e| e.action.as_str()).collect();
    assert_eq!(
        actions,
        vec![
            "schedule.create",
            "schedule.pause",
            "schedule.resume",
            "schedule.delete"
        ]
    );
    for e in &entries {
        assert_eq!(e.actor, "cli:owner");
        assert_eq!(e.resource.as_deref(), Some(job.id.as_str()));
    }
    assert_eq!(entries[0].details["name"], "audited");
}

// ---------------------------------------------------------------------------
// run-now (REST against the running server)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_now_posts_fire_now_to_server() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("POST", "/v1/admin/scheduler/jobs/job_x/fire-now")
        .with_status(202)
        .create_async()
        .await;

    run_now(&server.url(), "job_x").await.expect("fired");
    m.assert_async().await;
}

#[tokio::test]
async fn run_now_maps_404_and_503_to_teaching_errors() {
    let mut server = mockito::Server::new_async().await;
    let _m404 = server
        .mock("POST", "/v1/admin/scheduler/jobs/missing/fire-now")
        .with_status(404)
        .create_async()
        .await;
    let err = run_now(&server.url(), "missing")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("missing"), "{err}");

    let _m503 = server
        .mock("POST", "/v1/admin/scheduler/jobs/unwired/fire-now")
        .with_status(503)
        .create_async()
        .await;
    let err = run_now(&server.url(), "unwired")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.to_lowercase().contains("scheduler"), "{err}");
}

// ---------------------------------------------------------------------------
// formatting
// ---------------------------------------------------------------------------

#[tokio::test]
async fn format_table_shows_short_id_name_trigger_status_and_last_result() {
    let repo = InMemoryJobRepository::new();
    let runs = InMemoryJobRunRepository::new();
    let audit = RecordingAuditAppender::new();

    let job = create(&repo, &audit, create_args("tabular")).await.unwrap();
    runs.insert(finished_run(&job.id, JobRunStatus::Succeeded))
        .await
        .unwrap();
    set_enabled(&repo, &audit, &job.id, false).await.unwrap();

    let rows = list(&repo, &runs, 50).await.unwrap();
    let table = format_table(&rows);

    assert!(table.contains("ID"), "{table}");
    assert!(table.contains("NEXT FIRE"), "{table}");
    assert!(table.contains(&job.id[..12]), "short id shown: {table}");
    assert!(table.contains("tabular"), "{table}");
    assert!(table.contains("paused"), "{table}");
    assert!(table.contains("succeeded"), "{table}");
}

#[tokio::test]
async fn format_detail_includes_cron_payload_and_run_history() {
    let repo = InMemoryJobRepository::new();
    let runs = InMemoryJobRunRepository::new();
    let audit = RecordingAuditAppender::new();

    let job = create(&repo, &audit, create_args("verbose")).await.unwrap();
    runs.insert(finished_run(&job.id, JobRunStatus::Failed))
        .await
        .unwrap();

    let (got, history) = show(&repo, &runs, &job.id).await.unwrap();
    let detail = format_detail(&got, &history);

    assert!(detail.contains(&job.id), "{detail}");
    assert!(detail.contains("0 0 8 * * *"), "{detail}");
    assert!(detail.contains("scan the inbox"), "{detail}");
    assert!(detail.contains("failed"), "{detail}");
}
