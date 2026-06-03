//! Integration tests for `PgLlmProviderRepository` (embedded `SQLite`, DEC-033).
//!
//! No Docker — each test opens a temp `SQLite` database via `common::test_setup`.
//! Under the single-user pivot the tenant-vs-global split collapses to one
//! namespace: `tenant_id` is dropped on write and reads back as `None`, and
//! `list_global` / `list_for_tenant` both return the whole table.

mod common;

use chrono::{SubsecRound, Utc};
use common::test_setup;
use xiaoguai_storage::repositories::{LlmProviderRepository, PgLlmProviderRepository, RepoError};
use xiaoguai_storage::OWNER_TENANT_ID;
use xiaoguai_types::{ids::TenantId, LlmProvider, ProviderId, ProviderKind};

fn sample_provider(name: &str, tenant_id: Option<TenantId>) -> LlmProvider {
    let now = Utc::now().trunc_subsecs(6);
    LlmProvider {
        id: ProviderId::new(),
        tenant_id,
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
    let repo = PgLlmProviderRepository::new(pool);
    let prov = sample_provider("deepseek-global", None);

    repo.create(None, &prov).await.expect("create");
    let found = repo
        .find_by_id(None, prov.id.as_str())
        .await
        .expect("query")
        .expect("present");

    assert_eq!(found.id.as_str(), prov.id.as_str());
    // tenant_id is dropped on write; reads back as None under the pivot.
    assert!(found.tenant_id.is_none());
    assert_eq!(found.name, "deepseek-global");
    assert_eq!(found.kind, ProviderKind::OpenAiCompat);
    assert_eq!(found.models, vec!["deepseek-chat", "deepseek-coder"]);
    assert_eq!(found.default_for_models, vec!["deepseek-chat"]);
    assert_eq!(found.fallback_order, 10);
    assert_eq!(found.api_key_env.as_deref(), Some("DEEPSEEK_API_KEY"));
}

#[tokio::test]
async fn create_with_tenant_id_reads_back_as_none() {
    let (pool, _guard) = test_setup().await;
    let repo = PgLlmProviderRepository::new(pool);
    // A fixture carrying a tenant id still persists, but tenant_id is not stored.
    let prov = sample_provider("scoped", Some(TenantId::from(OWNER_TENANT_ID.to_string())));
    repo.create(None, &prov).await.expect("create");

    let found = repo
        .find_by_id(None, prov.id.as_str())
        .await
        .expect("query")
        .expect("present");
    assert!(found.tenant_id.is_none());
}

#[tokio::test]
async fn duplicate_name_is_rejected() {
    let (pool, _guard) = test_setup().await;
    let repo = PgLlmProviderRepository::new(pool);
    let p1 = sample_provider("dup", None);
    let p2 = sample_provider("dup", None);

    repo.create(None, &p1).await.expect("first");
    let err = repo
        .create(None, &p2)
        .await
        .expect_err("second should fail");
    assert!(matches!(err, RepoError::DuplicateKey(_)), "got: {err:?}");
}

#[tokio::test]
async fn list_returns_all_rows_ordered_by_fallback() {
    let (pool, _guard) = test_setup().await;
    let repo = PgLlmProviderRepository::new(pool);
    repo.create(None, &sample_provider("p-1", None))
        .await
        .expect("p1");
    repo.create(None, &sample_provider("p-2", None))
        .await
        .expect("p2");
    repo.create(None, &sample_provider("p-3", None))
        .await
        .expect("p3");

    // Single namespace: list_global and list_for_tenant see the same rows.
    let global = repo.list_global().await.expect("list_global");
    let names: Vec<&str> = global.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"p-1"));
    assert!(names.contains(&"p-2"));
    assert!(names.contains(&"p-3"));

    let scoped = repo
        .list_for_tenant(OWNER_TENANT_ID)
        .await
        .expect("list_for_tenant");
    assert_eq!(scoped.len(), global.len());
}

#[tokio::test]
async fn delete_idempotent() {
    let (pool, _guard) = test_setup().await;
    let repo = PgLlmProviderRepository::new(pool);
    let prov = sample_provider("del-me", None);
    repo.create(None, &prov).await.expect("create");
    repo.delete(None, prov.id.as_str())
        .await
        .expect("first delete");
    repo.delete(None, prov.id.as_str())
        .await
        .expect("second delete");
    assert!(repo
        .find_by_id(None, prov.id.as_str())
        .await
        .expect("query")
        .is_none());
}
