//! Unit tests for the `provider` subcommand business logic.
//!
//! These exercise the command functions directly with an in-memory repository
//! — fast, no Docker required. The PG path is covered by
//! `xiaoguai-storage/tests/llm_provider_repo.rs` (`#[ignore]`).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use xiaoguai_cli::commands::provider::{
    format_table, list, register, remove, update, ListArgs, RegisterArgs, RemoveArgs, UpdateArgs,
};
use xiaoguai_storage::repositories::{LlmProviderRepository, RepoError, RepoResult};
use xiaoguai_types::{LlmProvider, ProviderKind};

#[derive(Default, Clone)]
struct MemoryRepo {
    rows: Arc<Mutex<HashMap<String, LlmProvider>>>,
}

#[async_trait]
impl LlmProviderRepository for MemoryRepo {
    async fn create(&self, prov: &LlmProvider) -> RepoResult<()> {
        let mut rows = self.rows.lock();
        // Reject duplicate name to mirror the PG unique index.
        if rows.values().any(|r| r.name == prov.name) {
            return Err(RepoError::DuplicateKey(prov.name.clone()));
        }
        rows.insert(prov.id.as_str().to_string(), prov.clone());
        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> RepoResult<Option<LlmProvider>> {
        Ok(self.rows.lock().get(id).cloned())
    }

    async fn list(&self) -> RepoResult<Vec<LlmProvider>> {
        let mut out: Vec<LlmProvider> = self.rows.lock().values().cloned().collect();
        out.sort_by_key(|p| (p.fallback_order, p.created_at));
        Ok(out)
    }

    async fn delete(&self, id: &str) -> RepoResult<()> {
        self.rows.lock().remove(id);
        Ok(())
    }

    async fn update(&self, prov: &LlmProvider) -> RepoResult<()> {
        let mut rows = self.rows.lock();
        if !rows.contains_key(prov.id.as_str()) {
            return Err(RepoError::NotFound);
        }
        rows.insert(prov.id.as_str().to_string(), prov.clone());
        Ok(())
    }
}

fn args_ok(name: &str) -> RegisterArgs {
    RegisterArgs {
        name: name.into(),
        kind: "openai_compat".into(),
        endpoint: "https://api.example.com/v1".into(),
        models: vec!["m1".into()],
        default_for: vec![],
        fallback_order: 100,
        api_key_env: Some("EXAMPLE_KEY".into()),
        api_key: None,
    }
}

#[tokio::test]
async fn register_creates_and_returns_provider() {
    let repo = MemoryRepo::default();
    let p = register(&repo, args_ok("deepseek")).await.expect("ok");
    assert_eq!(p.name, "deepseek");
    assert_eq!(p.kind, ProviderKind::OpenAiCompat);
    assert!(p.id.as_str().starts_with("prov_"));

    let listed = list(&repo, ListArgs {}).await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id.as_str(), p.id.as_str());
}

#[tokio::test]
async fn register_rejects_unknown_kind() {
    let repo = MemoryRepo::default();
    let mut args = args_ok("anth");
    args.kind = "totally-bogus-kind".into();
    let err = register(&repo, args).await.expect_err("should fail");
    assert!(err.to_string().contains("unknown provider kind"));
}

#[tokio::test]
async fn register_rejects_empty_name() {
    let repo = MemoryRepo::default();
    let mut args = args_ok("ignored");
    args.name = "  ".into();
    let err = register(&repo, args).await.expect_err("should fail");
    assert!(err.to_string().contains("--name"));
}

#[tokio::test]
async fn register_rejects_empty_endpoint() {
    let repo = MemoryRepo::default();
    let mut args = args_ok("x");
    args.endpoint = String::new();
    let err = register(&repo, args).await.expect_err("should fail");
    assert!(err.to_string().contains("--endpoint"));
}

#[tokio::test]
async fn duplicate_within_scope_is_rejected() {
    let repo = MemoryRepo::default();
    register(&repo, args_ok("dup")).await.expect("first");
    let err = register(&repo, args_ok("dup")).await.expect_err("dup");
    let s = err.to_string();
    assert!(
        s.contains("duplicate") || s.contains("DuplicateKey"),
        "got: {s}"
    );
}

#[tokio::test]
async fn remove_is_idempotent() {
    let repo = MemoryRepo::default();
    let p = register(&repo, args_ok("rm")).await.expect("ok");
    remove(
        &repo,
        RemoveArgs {
            id: p.id.as_str().to_string(),
        },
    )
    .await
    .expect("first");
    remove(
        &repo,
        RemoveArgs {
            id: p.id.as_str().to_string(),
        },
    )
    .await
    .expect("second");
}

#[tokio::test]
async fn remove_rejects_empty_id() {
    let repo = MemoryRepo::default();
    let err = remove(
        &repo,
        RemoveArgs {
            id: "   ".to_string(),
        },
    )
    .await
    .expect_err("should fail");
    assert!(err.to_string().contains("--id"));
}

#[tokio::test]
async fn format_table_renders_headers_and_rows() {
    let repo = MemoryRepo::default();
    register(&repo, args_ok("deepseek")).await.expect("ok");
    let rows = list(&repo, ListArgs {}).await.expect("ok");
    let table = format_table(&rows);
    assert!(table.contains("ID"));
    assert!(table.contains("openai_compat"));
    assert!(table.contains("deepseek"));
}

#[tokio::test]
async fn register_with_api_key_stores_it_directly() {
    let repo = MemoryRepo::default();
    let mut args = args_ok("stored");
    args.api_key_env = None;
    args.api_key = Some("sk-cp-secret".into());
    let p = register(&repo, args).await.expect("ok");
    let got = repo
        .find_by_id(p.id.as_str())
        .await
        .expect("find")
        .expect("present");
    assert_eq!(got.api_key.as_deref(), Some("sk-cp-secret"));
    assert!(got.api_key_env.is_none());
}

fn update_for(id: &str) -> UpdateArgs {
    UpdateArgs {
        id: id.to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn update_changes_endpoint_and_default_for_but_keeps_id() {
    let repo = MemoryRepo::default();
    let p = register(&repo, args_ok("upd")).await.expect("ok");
    let updated = update(
        &repo,
        UpdateArgs {
            endpoint: Some("https://api.minimaxi.com".into()),
            default_for: Some(vec!["MiniMax-M2".into()]),
            ..update_for(p.id.as_str())
        },
    )
    .await
    .expect("update");
    assert_eq!(updated.id.as_str(), p.id.as_str());
    assert_eq!(updated.endpoint, "https://api.minimaxi.com");
    assert_eq!(updated.default_for_models, vec!["MiniMax-M2".to_string()]);
    // models untouched (only the fields we passed change).
    assert_eq!(updated.models, p.models);
}

#[tokio::test]
async fn update_can_set_api_key_and_clear_default_for() {
    let repo = MemoryRepo::default();
    let mut args = args_ok("key");
    args.default_for = vec!["m1".into()];
    let p = register(&repo, args).await.expect("ok");
    let updated = update(
        &repo,
        UpdateArgs {
            api_key: Some("sk-cp-new".into()),
            default_for: Some(vec![]), // explicit empty clears it
            ..update_for(p.id.as_str())
        },
    )
    .await
    .expect("update");
    assert_eq!(updated.api_key.as_deref(), Some("sk-cp-new"));
    assert!(updated.default_for_models.is_empty());
}

#[tokio::test]
async fn update_unknown_id_errors() {
    let repo = MemoryRepo::default();
    let err = update(&repo, update_for("prov_does_not_exist"))
        .await
        .expect_err("should fail");
    assert!(err.to_string().contains("no provider with id"));
}

#[tokio::test]
async fn update_rejects_empty_id() {
    let repo = MemoryRepo::default();
    let err = update(&repo, update_for("  "))
        .await
        .expect_err("should fail");
    assert!(err.to_string().contains("--id"));
}
