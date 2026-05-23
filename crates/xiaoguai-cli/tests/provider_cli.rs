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
    format_table, list, register, remove, ListArgs, RegisterArgs, RemoveArgs,
};
use xiaoguai_storage::repositories::{LlmProviderRepository, RepoError, RepoResult};
use xiaoguai_types::{LlmProvider, ProviderKind};

#[derive(Default, Clone)]
struct MemoryRepo {
    rows: Arc<Mutex<HashMap<String, LlmProvider>>>,
}

#[async_trait]
impl LlmProviderRepository for MemoryRepo {
    async fn create(&self, _tenant: Option<&str>, prov: &LlmProvider) -> RepoResult<()> {
        let key_scope = prov.tenant_id.as_ref().map(|t| t.as_str().to_string());
        let mut rows = self.rows.lock();
        // Reject duplicate (scope, name) to mirror the PG unique index.
        if rows.values().any(|r| {
            r.name == prov.name && r.tenant_id.as_ref().map(|t| t.as_str().to_string()) == key_scope
        }) {
            return Err(RepoError::DuplicateKey(format!(
                "({:?},{})",
                key_scope, prov.name
            )));
        }
        rows.insert(prov.id.as_str().to_string(), prov.clone());
        Ok(())
    }

    async fn find_by_id(&self, _tenant: Option<&str>, id: &str) -> RepoResult<Option<LlmProvider>> {
        Ok(self.rows.lock().get(id).cloned())
    }

    async fn list_global(&self) -> RepoResult<Vec<LlmProvider>> {
        let mut out: Vec<LlmProvider> = self
            .rows
            .lock()
            .values()
            .filter(|p| p.tenant_id.is_none())
            .cloned()
            .collect();
        out.sort_by_key(|p| (p.fallback_order, p.created_at));
        Ok(out)
    }

    async fn list_for_tenant(&self, tenant_id: &str) -> RepoResult<Vec<LlmProvider>> {
        let mut out: Vec<LlmProvider> = self
            .rows
            .lock()
            .values()
            .filter(|p| {
                p.tenant_id.is_none()
                    || p.tenant_id
                        .as_ref()
                        .is_some_and(|t| t.as_str() == tenant_id)
            })
            .cloned()
            .collect();
        out.sort_by_key(|p| (p.fallback_order, p.tenant_id.is_some(), p.created_at));
        Ok(out)
    }

    async fn delete(&self, _tenant: Option<&str>, id: &str) -> RepoResult<()> {
        self.rows.lock().remove(id);
        Ok(())
    }
}

fn args_ok(name: &str, tenant: Option<&str>) -> RegisterArgs {
    RegisterArgs {
        name: name.into(),
        kind: "openai_compat".into(),
        endpoint: "https://api.example.com/v1".into(),
        models: vec!["m1".into()],
        default_for: vec![],
        fallback_order: 100,
        api_key_env: Some("EXAMPLE_KEY".into()),
        tenant: tenant.map(str::to_string),
    }
}

#[tokio::test]
async fn register_creates_and_returns_provider() {
    let repo = MemoryRepo::default();
    let p = register(&repo, args_ok("deepseek", None))
        .await
        .expect("ok");
    assert_eq!(p.name, "deepseek");
    assert_eq!(p.kind, ProviderKind::OpenAiCompat);
    assert!(p.id.as_str().starts_with("prov_"));

    let listed = list(&repo, ListArgs { tenant: None }).await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id.as_str(), p.id.as_str());
}

#[tokio::test]
async fn register_rejects_unknown_kind() {
    let repo = MemoryRepo::default();
    let mut args = args_ok("anth", None);
    args.kind = "anthropic".into();
    let err = register(&repo, args).await.expect_err("should fail");
    assert!(err.to_string().contains("unknown provider kind"));
}

#[tokio::test]
async fn register_rejects_empty_name() {
    let repo = MemoryRepo::default();
    let mut args = args_ok("ignored", None);
    args.name = "  ".into();
    let err = register(&repo, args).await.expect_err("should fail");
    assert!(err.to_string().contains("--name"));
}

#[tokio::test]
async fn register_rejects_empty_endpoint() {
    let repo = MemoryRepo::default();
    let mut args = args_ok("x", None);
    args.endpoint = String::new();
    let err = register(&repo, args).await.expect_err("should fail");
    assert!(err.to_string().contains("--endpoint"));
}

#[tokio::test]
async fn duplicate_within_scope_is_rejected() {
    let repo = MemoryRepo::default();
    register(&repo, args_ok("dup", None)).await.expect("first");
    let err = register(&repo, args_ok("dup", None))
        .await
        .expect_err("dup");
    let s = err.to_string();
    assert!(
        s.contains("duplicate") || s.contains("DuplicateKey"),
        "got: {s}"
    );
}

#[tokio::test]
async fn list_for_tenant_includes_globals_and_tenant_rows() {
    let repo = MemoryRepo::default();
    register(&repo, args_ok("global", None)).await.expect("g");
    register(&repo, args_ok("alpha-only", Some("ten_alpha")))
        .await
        .expect("a");
    register(&repo, args_ok("beta-only", Some("ten_beta")))
        .await
        .expect("b");

    let rows = list(
        &repo,
        ListArgs {
            tenant: Some("ten_alpha".into()),
        },
    )
    .await
    .expect("ok");
    let names: Vec<&str> = rows.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"global"));
    assert!(names.contains(&"alpha-only"));
    assert!(!names.contains(&"beta-only"), "leaked beta: {names:?}");
}

#[tokio::test]
async fn remove_is_idempotent() {
    let repo = MemoryRepo::default();
    let p = register(&repo, args_ok("rm", None)).await.expect("ok");
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
    register(&repo, args_ok("deepseek", None))
        .await
        .expect("ok");
    let rows = list(&repo, ListArgs { tenant: None }).await.expect("ok");
    let table = format_table(&rows);
    assert!(table.contains("ID"));
    assert!(table.contains("global"));
    assert!(table.contains("openai_compat"));
    assert!(table.contains("deepseek"));
}
