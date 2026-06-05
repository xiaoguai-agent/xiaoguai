//! `xiaoguai provider {register,list,remove}` — administer the LLM provider
//! registry stored in `SQLite`.
//!
//! These functions take a `&dyn LlmProviderRepository` so unit tests can swap
//! in an in-memory implementation. The binary entry point in `main.rs`
//! constructs a `SqliteLlmProviderRepository` and calls them.
//!
//! Secrets policy: API keys arrive one of two ways, never on argv:
//! * `--api-key-env DEEPSEEK_API_KEY` — env-var indirection; the runtime reads
//!   the named variable when the backend is built. The key never touches the DB.
//! * `--api-key-stdin` — for headless / `pip install` deployments with no web
//!   UI: `main.rs` reads the key from **stdin** (so it stays out of shell
//!   history and argv) and stores it in the local single-user DB `api_key`
//!   column, exactly as the web admin Providers pane does. The runtime prefers
//!   a stored `api_key` over `api_key_env`.

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
    /// Directly-stored API key (read from stdin by `main.rs`, never argv).
    /// Persisted to the DB `api_key` column; the runtime prefers it over
    /// `api_key_env`. `None` for env-var or unauthenticated providers.
    pub api_key: Option<String>,
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
        // `--api-key-env` keeps the key out of the DB; `--api-key-stdin` stores
        // it here directly (same column the web admin pane writes).
        api_key: args.api_key,
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
pub struct UpdateArgs {
    pub id: String,
    /// `None` = leave unchanged. `Some` replaces the field.
    pub endpoint: Option<String>,
    pub models: Option<Vec<String>>,
    pub default_for: Option<Vec<String>>,
    pub fallback_order: Option<i32>,
    pub api_key_env: Option<String>,
    /// Directly-stored key (read from stdin by `main.rs`). `Some` overwrites
    /// the DB `api_key` column.
    pub api_key: Option<String>,
}

/// Update the mutable fields of an existing provider (matched by `--id`). Only
/// the fields supplied are changed; everything else keeps its current value.
/// Returns the updated record.
///
/// # Errors
/// Returns an error if `--id` is empty, no provider has that id, `--endpoint`
/// is given but blank, or the repository call fails.
pub async fn update(repo: &dyn LlmProviderRepository, args: UpdateArgs) -> Result<LlmProvider> {
    if args.id.trim().is_empty() {
        return Err(anyhow!("--id must not be empty"));
    }
    let mut prov = repo.find_by_id(&args.id).await?.ok_or_else(|| {
        anyhow!(
            "no provider with id '{}' — run `xiaoguai provider list` to see ids",
            args.id
        )
    })?;

    if let Some(endpoint) = args.endpoint {
        if endpoint.trim().is_empty() {
            return Err(anyhow!("--endpoint must not be empty"));
        }
        prov.endpoint = endpoint;
    }
    if let Some(models) = args.models {
        prov.models = models;
    }
    if let Some(default_for) = args.default_for {
        prov.default_for_models = default_for;
    }
    if let Some(order) = args.fallback_order {
        prov.fallback_order = order;
    }
    if let Some(env) = args.api_key_env {
        prov.api_key_env = Some(env);
    }
    if let Some(key) = args.api_key {
        prov.api_key = Some(key);
    }
    prov.updated_at = Utc::now();
    repo.update(&prov).await?;
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
