//! `xiaoguai provider {register,list,remove}` — administer the LLM provider
//! registry stored in Postgres.
//!
//! These functions take a `&dyn LlmProviderRepository` so unit tests can swap
//! in an in-memory implementation. The binary entry point in `main.rs`
//! constructs a `PgLlmProviderRepository` and calls them.
//!
//! Secrets policy: this command **never** accepts an API key directly. Callers
//! pass `--api-key-env DEEPSEEK_API_KEY` and the runtime reads the key from
//! the named environment variable when the backend is constructed. Keys never
//! enter the database, audit log, or shell history.

use anyhow::{anyhow, Result};
use chrono::Utc;
use xiaoguai_storage::repositories::LlmProviderRepository;
use xiaoguai_types::{ids::TenantId, LlmProvider, ProviderId, ProviderKind};

#[derive(Debug, Clone)]
pub struct RegisterArgs {
    pub name: String,
    pub kind: String,
    pub endpoint: String,
    pub models: Vec<String>,
    pub default_for: Vec<String>,
    pub fallback_order: i32,
    pub api_key_env: Option<String>,
    pub tenant: Option<String>,
}

/// Insert a new provider row and return the persisted record (with the
/// freshly-allocated id and timestamps).
///
/// # Errors
/// Returns an error if the provider kind string is not recognised, if
/// `--endpoint` or `--name` are empty, or if the repository `create` call
/// fails.
pub async fn register(repo: &dyn LlmProviderRepository, args: RegisterArgs) -> Result<LlmProvider> {
    let kind = ProviderKind::parse(&args.kind).ok_or_else(|| {
        anyhow!(
            "unknown provider kind '{}': expected 'ollama' or 'openai_compat'",
            args.kind
        )
    })?;
    if args.endpoint.trim().is_empty() {
        return Err(anyhow!("--endpoint must not be empty"));
    }
    if args.name.trim().is_empty() {
        return Err(anyhow!("--name must not be empty"));
    }

    let now = Utc::now();
    let tenant_guc = args.tenant.clone();
    let prov = LlmProvider {
        id: ProviderId::new(),
        tenant_id: args.tenant.map(TenantId::from),
        name: args.name,
        kind,
        endpoint: args.endpoint,
        models: args.models,
        default_for_models: args.default_for,
        fallback_order: args.fallback_order,
        api_key_env: args.api_key_env,
        created_at: now,
        updated_at: now,
        // Cost rates are not supplied via the register CLI; operators set
        // them by running the migration or via direct SQL UPDATE.
        cost_per_1k_input_usd: None,
        cost_per_1k_output_usd: None,
    };
    repo.create(tenant_guc.as_deref(), &prov).await?;
    Ok(prov)
}

#[derive(Debug, Clone, Default)]
pub struct ListArgs {
    /// `None` lists system-wide providers only. `Some(id)` lists globals +
    /// rows scoped to that tenant.
    pub tenant: Option<String>,
}

/// List LLM providers from the repository.
///
/// # Errors
/// Returns an error if the repository query fails.
pub async fn list(repo: &dyn LlmProviderRepository, args: ListArgs) -> Result<Vec<LlmProvider>> {
    let rows = match args.tenant {
        Some(t) => repo.list_for_tenant(&t).await?,
        None => repo.list_global().await?,
    };
    Ok(rows)
}

#[derive(Debug, Clone)]
pub struct RemoveArgs {
    pub id: String,
}

/// Remove an LLM provider by ID.
///
/// # Errors
/// Returns an error if `--id` is empty or if the repository `delete` call
/// fails.
pub async fn remove(repo: &dyn LlmProviderRepository, args: RemoveArgs) -> Result<()> {
    if args.id.trim().is_empty() {
        return Err(anyhow!("--id must not be empty"));
    }
    // Admin CLI: caller may not know the tenant; rely on superuser/owner
    // bypass for RLS. v0.6.2 should add a `--tenant` flag to scope deletes.
    repo.delete(None, &args.id).await?;
    Ok(())
}

/// Render a provider list as a fixed-width text table suitable for human
/// consumption. Pure function so unit-testable.
#[must_use]
pub fn format_table(rows: &[LlmProvider]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    out.push_str(
        "ID                                     SCOPE       KIND           NAME             ENDPOINT\n",
    );
    for p in rows {
        let scope = p
            .tenant_id
            .as_ref()
            .map_or_else(|| "global".to_string(), |t| t.as_str().to_string());
        // `write!` to a String is infallible.
        let _ = writeln!(
            out,
            "{:38} {:11} {:14} {:16} {}",
            p.id.as_str(),
            scope,
            p.kind.as_str(),
            p.name,
            p.endpoint
        );
    }
    out
}
