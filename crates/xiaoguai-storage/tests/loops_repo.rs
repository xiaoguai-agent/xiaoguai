//! /loop L1: integration tests for `LoopStore` / `SqliteLoopRepository`
//! (LLD-LOOP-001 §9 gate: "repo round-trip").
//!
//! Embedded `SQLite` (DEC-033) — each test opens a temp database via
//! `common::test_setup`.

mod common;

use chrono::{Duration, Utc};
use common::test_setup;
use uuid::Uuid;
use xiaoguai_storage::repositories::{
    LoopRow, LoopStatus, LoopStore, RepoError, SqliteLoopRepository,
};

fn make_loop(session_id: &str) -> LoopRow {
    let now = Utc::now();
    LoopRow {
        id: Uuid::new_v4(),
        session_id: session_id.to_string(),
        prompt: "check the CI run and report regressions".to_string(),
        pacing_kind: xiaoguai_storage::repositories::PacingKind::Fixed,
        interval_secs: 300,
        min_interval_secs: 10,
        max_interval_secs: 3600,
        max_ticks: 50,
        ttl_secs: 86_400,
        max_total_tokens: 500_000,
        status: LoopStatus::Active,
        created_by: "usr_a".to_string(),
        created_at: now,
        expires_at: now + Duration::seconds(86_400),
        next_tick_at: now + Duration::seconds(300),
        ticks_run: 0,
        consecutive_failures: 0,
        last_error: None,
    }
}

#[tokio::test]
async fn insert_get_round_trip() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLoopRepository::new(pool);
    let row = make_loop("sess_1");

    repo.insert(&row).await.expect("insert");
    let got = repo.get(row.id).await.expect("get").expect("row exists");

    assert_eq!(got.id, row.id);
    assert_eq!(got.session_id, "sess_1");
    assert_eq!(got.prompt, row.prompt);
    assert_eq!(got.interval_secs, 300);
    assert_eq!(got.max_ticks, 50);
    assert_eq!(got.ttl_secs, 86_400);
    assert_eq!(got.status, LoopStatus::Active);
    assert_eq!(got.created_by, "usr_a");
    assert_eq!(got.ticks_run, 0);
    assert_eq!(got.consecutive_failures, 0);
    assert!(got.last_error.is_none());
    // Timestamps survive the round-trip to second precision.
    assert!((got.next_tick_at - row.next_tick_at).num_seconds().abs() <= 1);
    assert!((got.expires_at - row.expires_at).num_seconds().abs() <= 1);
}

#[tokio::test]
async fn get_unknown_returns_none() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLoopRepository::new(pool);
    assert!(repo.get(Uuid::new_v4()).await.expect("get").is_none());
}

#[tokio::test]
async fn second_live_loop_on_same_session_is_duplicate_key() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLoopRepository::new(pool);
    repo.insert(&make_loop("sess_1")).await.expect("first");

    let err = repo.insert(&make_loop("sess_1")).await.expect_err("second");
    assert!(
        matches!(err, RepoError::DuplicateKey(_)),
        "expected DuplicateKey, got {err:?}"
    );

    // A different session is unaffected.
    repo.insert(&make_loop("sess_2"))
        .await
        .expect("other session");
}

#[tokio::test]
async fn terminal_loop_frees_the_session_slot() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLoopRepository::new(pool);
    let first = make_loop("sess_1");
    repo.insert(&first).await.expect("insert");

    let moved = repo
        .terminalise(first.id, LoopStatus::Cancelled, Some("operator cancel"))
        .await
        .expect("terminalise");
    assert!(moved);

    // The slot is free again — a new loop can be created.
    repo.insert(&make_loop("sess_1"))
        .await
        .expect("replacement");
}

#[tokio::test]
async fn terminalise_is_immutable_once_terminal() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLoopRepository::new(pool);
    let row = make_loop("sess_1");
    repo.insert(&row).await.expect("insert");

    assert!(repo
        .terminalise(
            row.id,
            LoopStatus::Failed,
            Some("five consecutive failures")
        )
        .await
        .expect("first terminalise"));
    // Double-terminalise (e.g. cancel racing auto-fail) returns false and
    // does not overwrite the terminal status.
    assert!(!repo
        .terminalise(row.id, LoopStatus::Cancelled, None)
        .await
        .expect("second terminalise"));

    let got = repo.get(row.id).await.expect("get").expect("row");
    assert_eq!(got.status, LoopStatus::Failed);
    assert_eq!(got.last_error.as_deref(), Some("five consecutive failures"));
}

#[tokio::test]
async fn terminalise_rejects_non_terminal_target() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLoopRepository::new(pool);
    let row = make_loop("sess_1");
    repo.insert(&row).await.expect("insert");

    let err = repo
        .terminalise(row.id, LoopStatus::Paused, None)
        .await
        .expect_err("paused is not terminal");
    assert!(matches!(err, RepoError::InvalidArgument(_)));
}

#[tokio::test]
async fn pause_moves_active_to_paused_and_keeps_the_slot() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLoopRepository::new(pool);
    let row = make_loop("sess_1");
    repo.insert(&row).await.expect("insert");

    assert!(repo
        .pause(row.id, Some("waiting on human"))
        .await
        .expect("pause"));
    let got = repo.get(row.id).await.expect("get").expect("row");
    assert_eq!(got.status, LoopStatus::Paused);
    assert_eq!(got.last_error.as_deref(), Some("waiting on human"));

    // Paused still holds the one-per-session slot.
    let err = repo
        .insert(&make_loop("sess_1"))
        .await
        .expect_err("blocked");
    assert!(matches!(err, RepoError::DuplicateKey(_)));

    // Pausing again is a no-op (not active any more).
    assert!(!repo.pause(row.id, None).await.expect("re-pause"));
    // A paused loop can still be cancelled.
    assert!(repo
        .terminalise(row.id, LoopStatus::Cancelled, None)
        .await
        .expect("cancel paused"));
}

#[tokio::test]
async fn resume_moves_paused_back_to_active() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLoopRepository::new(pool);
    let mut row = make_loop("sess_1");
    row.consecutive_failures = 3;
    row.last_error = Some("boom".into());
    repo.insert(&row).await.expect("insert");
    repo.pause(row.id, Some("waiting")).await.expect("pause");

    let next = Utc::now() + Duration::seconds(300);
    assert!(repo.resume(row.id, next).await.expect("resume"));
    let got = repo.get(row.id).await.expect("get").expect("row");
    assert_eq!(got.status, LoopStatus::Active);
    assert_eq!(
        got.consecutive_failures, 0,
        "resume resets the failure counter"
    );
    assert!(got.last_error.is_none(), "resume clears last_error");

    // Resuming an active loop is a no-op (only paused resumes).
    assert!(!repo.resume(row.id, next).await.expect("re-resume"));
    // Resuming a terminal loop is a no-op.
    repo.terminalise(row.id, LoopStatus::Cancelled, None)
        .await
        .expect("cancel");
    assert!(!repo.resume(row.id, next).await.expect("resume terminal"));
}

#[tokio::test]
async fn record_tick_updates_bookkeeping_for_active_rows_only() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLoopRepository::new(pool);
    let row = make_loop("sess_1");
    repo.insert(&row).await.expect("insert");

    let next = Utc::now() + Duration::seconds(600);
    assert!(repo
        .record_tick(row.id, next, 1, 0, None)
        .await
        .expect("tick on active row"));

    let got = repo.get(row.id).await.expect("get").expect("row");
    assert_eq!(got.ticks_run, 1);

    // Once terminal, late bookkeeping must not revive the row.
    repo.terminalise(row.id, LoopStatus::Cancelled, None)
        .await
        .expect("terminalise");
    assert!(!repo
        .record_tick(row.id, next, 2, 1, Some("late"))
        .await
        .expect("tick on terminal row"));
    let got = repo.get(row.id).await.expect("get").expect("row");
    assert_eq!(got.ticks_run, 1, "terminal row must be untouched");
    assert_eq!(got.status, LoopStatus::Cancelled);
}

#[tokio::test]
async fn list_active_returns_only_active_ordered_by_next_tick() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLoopRepository::new(pool);

    let mut early = make_loop("sess_early");
    early.next_tick_at = Utc::now() + Duration::seconds(10);
    let mut late = make_loop("sess_late");
    late.next_tick_at = Utc::now() + Duration::seconds(900);
    let done = make_loop("sess_done");

    repo.insert(&late).await.expect("late");
    repo.insert(&early).await.expect("early");
    repo.insert(&done).await.expect("done");
    repo.terminalise(done.id, LoopStatus::Done, None)
        .await
        .expect("terminalise");

    let active = repo.list_active().await.expect("list_active");
    assert_eq!(active.len(), 2);
    assert_eq!(active[0].id, early.id, "ordered by next_tick_at ASC");
    assert_eq!(active[1].id, late.id);

    // `list` keeps the full history, terminal rows included.
    assert_eq!(repo.list().await.expect("list").len(), 3);
}
