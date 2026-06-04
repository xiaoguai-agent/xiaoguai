//! Integration tests for `SqliteLlmProviderRepository` (embedded `SQLite`, DEC-033).
//!
//! No Docker — each test opens a temp `SQLite` database via `common::test_setup`.
//! Single-owner deployment: `list` returns the whole table.

mod common;

use chrono::{SubsecRound, Utc};
use common::test_setup;
use xiaoguai_storage::repositories::{
    LlmProviderRepository, RepoError, SqliteLlmProviderRepository,
};
use xiaoguai_types::{LlmProvider, ProviderId, ProviderKind};

fn sample_provider(name: &str) -> LlmProvider {
    let now = Utc::now().trunc_subsecs(6);
    LlmProvider {
        id: ProviderId::new(),
        name: name.to_string(),
        kind: ProviderKind::OpenAiCompat,
        endpoint: "https://api.deepseek.com/v1".to_string(),
        models: vec!["deepseek-chat".into(), "deepseek-coder".into()],
        default_for_models: vec!["deepseek-chat".into()],
        fallback_order: 10,
        api_key_env: Some("DEEPSEEK_API_KEY".into()),
        api_key: None,
        created_at: now,
        updated_at: now,
        cost_per_1k_input_usd: Some(0.27),
        cost_per_1k_output_usd: Some(1.10),
    }
}

#[tokio::test]
async fn create_and_find_by_id() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLlmProviderRepository::new(pool);
    let prov = sample_provider("deepseek-global");

    repo.create(&prov).await.expect("create");
    let found = repo
        .find_by_id(prov.id.as_str())
        .await
        .expect("query")
        .expect("present");

    assert_eq!(found.id.as_str(), prov.id.as_str());
    assert_eq!(found.name, "deepseek-global");
    assert_eq!(found.kind, ProviderKind::OpenAiCompat);
    assert_eq!(found.models, vec!["deepseek-chat", "deepseek-coder"]);
    assert_eq!(found.default_for_models, vec!["deepseek-chat"]);
    assert_eq!(found.fallback_order, 10);
    assert_eq!(found.api_key_env.as_deref(), Some("DEEPSEEK_API_KEY"));
}

#[tokio::test]
async fn duplicate_name_is_rejected() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLlmProviderRepository::new(pool);
    let p1 = sample_provider("dup");
    let p2 = sample_provider("dup");

    repo.create(&p1).await.expect("first");
    let err = repo.create(&p2).await.expect_err("second should fail");
    assert!(matches!(err, RepoError::DuplicateKey(_)), "got: {err:?}");
}

#[tokio::test]
async fn list_returns_all_rows_ordered_by_fallback() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLlmProviderRepository::new(pool);
    repo.create(&sample_provider("p-1")).await.expect("p1");
    repo.create(&sample_provider("p-2")).await.expect("p2");
    repo.create(&sample_provider("p-3")).await.expect("p3");

    let rows = repo.list().await.expect("list");
    let names: Vec<&str> = rows.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"p-1"));
    assert!(names.contains(&"p-2"));
    assert!(names.contains(&"p-3"));
}

#[tokio::test]
async fn delete_idempotent() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLlmProviderRepository::new(pool);
    let prov = sample_provider("del-me");
    repo.create(&prov).await.expect("create");
    repo.delete(prov.id.as_str()).await.expect("first delete");
    repo.delete(prov.id.as_str()).await.expect("second delete");
    assert!(repo
        .find_by_id(prov.id.as_str())
        .await
        .expect("query")
        .is_none());
}
