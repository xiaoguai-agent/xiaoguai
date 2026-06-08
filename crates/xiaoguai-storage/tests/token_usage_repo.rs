//! Integration tests for `SqliteTokenUsageRepository` (embedded `SQLite`, DEC-033).
//!
//! No Docker — each test opens a temp `SQLite` database via `common::test_setup`.
//! Single-owner deployment: `list` returns the whole ledger.

mod common;

use chrono::{SubsecRound, Utc};
use common::test_setup;
use xiaoguai_storage::repositories::{
    SqliteTokenUsageRepository, TokenUsageEntry, TokenUsageRepository,
};

fn sample_entry(prompt: i32, completion: i32) -> TokenUsageEntry {
    TokenUsageEntry {
        ts: Utc::now().trunc_subsecs(6),
        user_id: Some("usr_test".into()),
        session_id: Some("sess_test".into()),
        provider_id: "prov_test".into(),
        model: "qwen2.5".into(),
        prompt_tokens: Some(prompt),
        completion_tokens: Some(completion),
        total_tokens: Some(prompt + completion),
        request_id: Some("req_001".into()),
    }
}

#[tokio::test]
async fn batch_insert_and_list() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteTokenUsageRepository::new(pool);

    let batch = vec![
        sample_entry(10, 20),
        sample_entry(5, 15),
        sample_entry(100, 200),
    ];
    repo.record_batch(&batch).await.expect("batch insert");

    let listed = repo.list(10).await.expect("list");
    assert_eq!(listed.len(), 3);
}

#[tokio::test]
async fn empty_batch_is_noop() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteTokenUsageRepository::new(pool);
    repo.record_batch(&[]).await.expect("empty batch");
    let listed = repo.list(10).await.expect("list");
    assert!(listed.is_empty());
}

#[tokio::test]
async fn null_token_counts_are_stored() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteTokenUsageRepository::new(pool);

    let entry = TokenUsageEntry {
        ts: Utc::now().trunc_subsecs(6),
        user_id: None,
        session_id: None,
        provider_id: "prov_x".into(),
        model: "qwen2.5".into(),
        prompt_tokens: None,
        completion_tokens: None,
        total_tokens: None,
        request_id: None,
    };
    repo.record_batch(&[entry]).await.expect("insert");
    let listed = repo.list(1).await.expect("list");
    assert_eq!(listed.len(), 1);
    assert!(listed[0].entry.prompt_tokens.is_none());
    assert!(listed[0].entry.completion_tokens.is_none());
    assert!(listed[0].entry.total_tokens.is_none());
}

#[tokio::test]
async fn list_respects_limit() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteTokenUsageRepository::new(pool);
    let batch: Vec<_> = (0..5).map(|i| sample_entry(i, i)).collect();
    repo.record_batch(&batch).await.expect("batch");
    let listed = repo.list(3).await.expect("list");
    assert_eq!(listed.len(), 3);
}

#[tokio::test]
async fn session_total_since_sums_only_the_session_after_the_cutoff() {
    use chrono::Duration;
    let (pool, _guard) = test_setup().await;
    let repo = SqliteTokenUsageRepository::new(pool);

    let now = Utc::now().trunc_subsecs(6);
    let mk = |session: &str, ts, total: Option<i32>| TokenUsageEntry {
        ts,
        user_id: Some("u".into()),
        session_id: Some(session.into()),
        provider_id: "p".into(),
        model: "m".into(),
        prompt_tokens: None,
        completion_tokens: None,
        total_tokens: total,
        request_id: None,
    };
    repo.record_batch(&[
        mk("loop_sess", now + Duration::seconds(1), Some(100)),
        mk("loop_sess", now + Duration::seconds(2), Some(250)),
        // A NULL total contributes 0.
        mk("loop_sess", now + Duration::seconds(3), None),
        // Before the cutoff — excluded.
        mk("loop_sess", now - Duration::seconds(60), Some(9999)),
        // A different session — excluded.
        mk("other_sess", now + Duration::seconds(1), Some(5000)),
    ])
    .await
    .expect("insert");

    let total = repo
        .session_total_since("loop_sess", now)
        .await
        .expect("sum");
    assert_eq!(total, 350, "only this session's rows at/after the cutoff");

    // Unknown session → 0.
    assert_eq!(repo.session_total_since("nope", now).await.expect("sum"), 0);
}
