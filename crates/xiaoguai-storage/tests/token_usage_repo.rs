//! Integration tests for `PgTokenUsageRepository`.
//!
//! `#[ignore = "requires Docker"]` — run via
//! `cargo test -p xiaoguai-storage --test token_usage_repo -- --ignored`.
//!
//! The repository is multi-tenant but does **not** insert tenants/users/
//! sessions as foreign keys — `token_usage` is an append-only ledger, the
//! denormalised ids accommodate "tenant deleted, audit row stays" scenarios.

mod common;

use chrono::{SubsecRound, Utc};
use xiaoguai_storage::repositories::{
    PgTokenUsageRepository, TokenUsageEntry, TokenUsageRepository,
};

fn sample_entry(tenant: &str, prompt: i32, completion: i32) -> TokenUsageEntry {
    TokenUsageEntry {
        ts: Utc::now().trunc_subsecs(6),
        tenant_id: tenant.into(),
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
#[ignore = "requires Docker"]
async fn batch_insert_and_list() {
    let (pool, _pg) = common::test_setup().await;
    let repo = PgTokenUsageRepository::new(pool);

    let batch = vec![
        sample_entry("ten_a", 10, 20),
        sample_entry("ten_a", 5, 15),
        sample_entry("ten_b", 100, 200),
    ];
    repo.record_batch(&batch).await.expect("batch insert");

    let listed_a = repo.list_for_tenant("ten_a", 10).await.expect("list a");
    assert_eq!(listed_a.len(), 2);
    assert!(listed_a.iter().all(|r| r.entry.tenant_id == "ten_a"));

    let listed_b = repo.list_for_tenant("ten_b", 10).await.expect("list b");
    assert_eq!(listed_b.len(), 1);
    assert_eq!(listed_b[0].entry.tenant_id, "ten_b");
    assert_eq!(listed_b[0].entry.prompt_tokens, Some(100));
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn empty_batch_is_noop() {
    let (pool, _pg) = common::test_setup().await;
    let repo = PgTokenUsageRepository::new(pool);
    repo.record_batch(&[]).await.expect("empty batch");
    let listed = repo.list_for_tenant("ten_any", 10).await.expect("list");
    assert!(listed.is_empty());
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn null_token_counts_are_stored() {
    let (pool, _pg) = common::test_setup().await;
    let repo = PgTokenUsageRepository::new(pool);

    let entry = TokenUsageEntry {
        ts: Utc::now().trunc_subsecs(6),
        tenant_id: "ten_x".into(),
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
    let listed = repo.list_for_tenant("ten_x", 1).await.expect("list");
    assert_eq!(listed.len(), 1);
    assert!(listed[0].entry.prompt_tokens.is_none());
    assert!(listed[0].entry.completion_tokens.is_none());
    assert!(listed[0].entry.total_tokens.is_none());
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn list_respects_limit() {
    let (pool, _pg) = common::test_setup().await;
    let repo = PgTokenUsageRepository::new(pool);
    let batch: Vec<_> = (0..5).map(|i| sample_entry("ten_l", i, i)).collect();
    repo.record_batch(&batch).await.expect("batch");
    let listed = repo.list_for_tenant("ten_l", 3).await.expect("list");
    assert_eq!(listed.len(), 3);
}
