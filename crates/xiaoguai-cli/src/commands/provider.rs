//! `xiaoguai provider {register,list,remove}` — administer the LLM provider
//! registry stored in `SQLite`.
//!
//! These functions take a `&dyn LlmProviderRepository` so unit tests can swap
//! in an in-memory implementation. The binary entry point in `main.rs`
//! constructs a `SqliteLlmProviderRepository` and calls them.
//!
//! Secrets policy: this command **never** accepts an API key directly. Callers
//! pass `--api-key-env DEEPSEEK_API_KEY` and the runtime reads the key from
//! the named environment variable when the backend is constructed. Keys never
//! enter the database, audit log, or shell history.

use anyhow::{anyhow, Result};
use chrono::Utc;
use xiaoguai_storage::repositories::LlmProviderRepository;
use xiaoguai_types::{LlmProvider, ProviderId, ProviderKind};

#[derive(Debug, Clone)]
pub struct RegisterArgs {
    pub name: String,
    pub kind: String,
    pub endpoint: String,
    pub models: Vec<String>,
    pub default_for: Vec<String>,
    pub fallback_order: i32,
    pub api_key_env: Option<String>,
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
            "unknown provider kind '{}': expected one of 'ollama', 'openai_compat', \
             'anthropic', 'gemini', 'bedrock', 'azure_openai', 'mistral', 'groq'",
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
    let prov = LlmProvider {
        id: ProviderId::new(),
        name: args.name,
        kind,
        endpoint: args.endpoint,
        models: args.models,
        default_for_models: args.default_for,
        fallback_order: args.fallback_order,
        api_key_env: args.api_key_env,
        // The register CLI uses env-var indirection; a directly-stored key is a
        // web-UI-only path (POST /v1/admin/providers).
        api_key: None,
        created_at: now,
        updated_at: now,
        // Cost rates are not supplied via the register CLI; operators set
        // them by running the migration or via direct SQL UPDATE.
        cost_per_1k_input_usd: None,
        cost_per_1k_output_usd: None,
    };
    repo.create(&prov).await?;
    Ok(prov)
}

#[derive(Debug, Clone, Default)]
pub struct ListArgs {}

/// List LLM providers from the repository.
///
/// # Errors
/// Returns an error if the repository query fails.
pub async fn list(repo: &dyn LlmProviderRepository, _args: ListArgs) -> Result<Vec<LlmProvider>> {
    Ok(repo.list().await?)
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
    repo.delete(&args.id).await?;
    Ok(())
}

/// Render a provider list as a fixed-width text table suitable for human
/// consumption. Pure function so unit-testable.
#[must_use]
pub fn format_table(rows: &[LlmProvider]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    out.push_str(
        "ID                                     KIND           NAME             ENDPOINT\n",
    );
    for p in rows {
        // `write!` to a String is infallible.
        let _ = writeln!(
            out,
            "{:38} {:14} {:16} {}",
            p.id.as_str(),
            p.kind.as_str(),
            p.name,
            p.endpoint
        );
    }
    out
}
