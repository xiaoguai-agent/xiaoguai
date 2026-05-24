//! Integration tests for `PgLlmProviderRepository`.
//!
//! Marked `#[ignore = "requires Docker"]` since they boot a Postgres container
//! via testcontainers. Run with
//! `cargo test -p xiaoguai-storage --test llm_provider_repo -- --ignored`.

mod common;

use chrono::{SubsecRound, Utc};
use xiaoguai_storage::repositories::{
    LlmProviderRepository, PgLlmProviderRepository, PgTenantRepository, RepoError, TenantRepository,
};
use xiaoguai_types::{ids::TenantId, LlmProvider, ProviderId, ProviderKind, Tenant, TenantStatus};

fn sample_tenant(name: &str) -> Tenant {
    Tenant {
        id: TenantId::new(),
        name: name.to_string(),
        display_name: format!("Display {name}"),
        created_at: Utc::now().trunc_subsecs(6),
        status: TenantStatus::Active,
    }
}

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
        created_at: now,
        updated_at: now,
        cost_per_1k_input_usd: Some(0.27),
        cost_per_1k_output_usd: Some(1.10),
    }
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn create_and_find_by_id_global_provider() {
    let (pool, _pg) = common::test_setup().await;
    let repo = PgLlmProviderRepository::new(pool);
    let prov = sample_provider("deepseek-global", None);

    repo.create(None, &prov).await.expect("create");
    let found = repo
        .find_by_id(None, prov.id.as_str())
        .await
        .expect("query")
        .expect("present");

    assert_eq!(found.id.as_str(), prov.id.as_str());
    assert!(found.tenant_id.is_none());
    assert_eq!(found.name, "deepseek-global");
    assert_eq!(found.kind, ProviderKind::OpenAiCompat);
    assert_eq!(found.models, vec!["deepseek-chat", "deepseek-coder"]);
    assert_eq!(found.default_for_models, vec!["deepseek-chat"]);
    assert_eq!(found.fallback_order, 10);
    assert_eq!(found.api_key_env.as_deref(), Some("DEEPSEEK_API_KEY"));
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn create_tenant_scoped_provider() {
    let (pool, _pg) = common::test_setup().await;
    let tenant_repo = PgTenantRepository::new(pool.clone());
    let tenant = sample_tenant("alpha");
    tenant_repo.create(&tenant).await.expect("tenant");

    let prov_repo = PgLlmProviderRepository::new(pool);
    let prov = sample_provider("alpha-deepseek", Some(tenant.id.clone()));
    prov_repo.create(None, &prov).await.expect("create");

    let found = prov_repo
        .find_by_id(None, prov.id.as_str())
        .await
        .expect("query")
        .expect("present");
    assert_eq!(
        found.tenant_id.as_ref().map(AsRef::as_ref),
        Some(tenant.id.as_str())
    );
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn duplicate_name_in_same_scope_is_rejected() {
    let (pool, _pg) = common::test_setup().await;
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
#[ignore = "requires Docker"]
async fn same_name_allowed_across_different_scopes() {
    let (pool, _pg) = common::test_setup().await;
    let tenant_repo = PgTenantRepository::new(pool.clone());
    let t1 = sample_tenant("t1");
    let t2 = sample_tenant("t2");
    tenant_repo.create(&t1).await.expect("t1");
    tenant_repo.create(&t2).await.expect("t2");

    let repo = PgLlmProviderRepository::new(pool);
    let global = sample_provider("provider-a", None);
    let in_t1 = sample_provider("provider-a", Some(t1.id.clone()));
    let in_t2 = sample_provider("provider-a", Some(t2.id.clone()));

    repo.create(None, &global).await.expect("global");
    repo.create(None, &in_t1).await.expect("t1");
    repo.create(None, &in_t2).await.expect("t2");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn list_for_tenant_returns_globals_plus_tenant_rows() {
    let (pool, _pg) = common::test_setup().await;
    let tenant_repo = PgTenantRepository::new(pool.clone());
    let t1 = sample_tenant("t1");
    let t2 = sample_tenant("t2");
    tenant_repo.create(&t1).await.expect("t1");
    tenant_repo.create(&t2).await.expect("t2");

    let repo = PgLlmProviderRepository::new(pool);
    repo.create(None, &sample_provider("global-1", None))
        .await
        .expect("g1");
    repo.create(None, &sample_provider("t1-only", Some(t1.id.clone())))
        .await
        .expect("t1");
    repo.create(None, &sample_provider("t2-only", Some(t2.id.clone())))
        .await
        .expect("t2");

    let listed = repo.list_for_tenant(t1.id.as_str()).await.expect("list");
    let names: Vec<&str> = listed.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"global-1"), "missing global: {names:?}");
    assert!(names.contains(&"t1-only"), "missing t1 row: {names:?}");
    assert!(!names.contains(&"t2-only"), "leaked t2 row: {names:?}");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn list_global_returns_only_global_rows() {
    let (pool, _pg) = common::test_setup().await;
    let tenant_repo = PgTenantRepository::new(pool.clone());
    let t = sample_tenant("t");
    tenant_repo.create(&t).await.expect("t");

    let repo = PgLlmProviderRepository::new(pool);
    repo.create(None, &sample_provider("global-1", None))
        .await
        .expect("g1");
    repo.create(None, &sample_provider("global-2", None))
        .await
        .expect("g2");
    repo.create(None, &sample_provider("t-row", Some(t.id.clone())))
        .await
        .expect("t-row");

    let listed = repo.list_global().await.expect("list");
    let names: Vec<&str> = listed.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"global-1"));
    assert!(names.contains(&"global-2"));
    assert!(!names.contains(&"t-row"), "leaked tenant row: {names:?}");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn delete_idempotent() {
    let (pool, _pg) = common::test_setup().await;
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
