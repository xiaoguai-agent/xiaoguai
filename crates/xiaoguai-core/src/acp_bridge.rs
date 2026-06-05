//! Wires the ACP stdio adapter (`xiaoguai-acp`, DEC-038 / `LLD-ACP-001`) to the
//! shared agent runtime.
//!
//! `xiaoguai acp` exposes the agent loop to ACP-speaking editors over the
//! process's stdio: newline-delimited JSON-RPC on stdin/stdout, logs on stderr
//! (the caller must point the tracing subscriber at stderr — see the CLI's
//! `main`). It builds a [`RuntimeContext`] the same way `run_serve` builds its
//! backend (LLM router from the provider registry, with a `MockBackend`
//! fallback), then serves until stdin closes.

use std::sync::Arc;

use anyhow::{Context, Result};
use xiaoguai_acp::{serve, AcpDelegate, RuntimeDelegate};
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_config::Settings;
use xiaoguai_llm::{build_router, LlmBackend, MockBackend, OsEnvResolver};
use xiaoguai_runtime::RuntimeContext;
use xiaoguai_storage::db;
use xiaoguai_storage::repositories::{LlmProviderRepository, SqliteLlmProviderRepository};

/// Serve the Agent Client Protocol over stdio until EOF.
///
/// # Errors
/// Returns an error if the store cannot be opened/migrated or the transport
/// fails unrecoverably.
pub async fn run_acp(settings: &Settings) -> Result<()> {
    let ctx = build_runtime_context(settings).await?;
    let delegate: Arc<dyn AcpDelegate> = Arc::new(RuntimeDelegate::new(ctx));
    tracing::info!("acp: serving Agent Client Protocol over stdio");
    serve(delegate, tokio::io::stdin(), tokio::io::stdout())
        .await
        .context("acp serve loop")
}

/// Assemble the runtime context: LLM backend (router or mock) + an (initially
/// empty) toolbox + agent defaults. Mirrors the backend selection in
/// [`crate::run_serve`]; coding-tool registration into the toolbox is the
/// deferred item per `LLD-ACP-001` §6.
async fn build_runtime_context(settings: &Settings) -> Result<RuntimeContext> {
    let pool = db::connect(&settings.database.url, settings.database.max_connections)
        .await
        .context("acp: db connect")?;
    db::migrate(&pool).await.context("acp: db migrate")?;

    let provider_repo = SqliteLlmProviderRepository::new(pool.clone());
    let mut rows = provider_repo.list().await.context("acp: list providers")?;
    // Same OLLAMA_HOST repoint as `run_serve`, so a local GPU box is honoured
    // without a SQL change.
    if let Ok(host) = std::env::var("OLLAMA_HOST") {
        let host = host.trim();
        if !host.is_empty() {
            for r in rows.iter_mut().filter(|r| r.id.as_str() == "ollama-local") {
                r.endpoint = host.to_string();
            }
        }
    }

    let (backend, default_model): (Arc<dyn LlmBackend>, String) = if rows.is_empty() {
        tracing::warn!(
            "acp: llm_providers table is empty — using MockBackend. \
             Register one via `xiaoguai provider register`."
        );
        (
            Arc::new(MockBackend::with_response(
                "No LLM providers configured. Register one via `xiaoguai provider register`.",
            )),
            "mock".to_string(),
        )
    } else {
        let (router, report) = build_router(&rows, &OsEnvResolver);
        for w in &report.warnings {
            tracing::warn!(warning = %w, "acp: llm router build");
        }
        let default_model = rows
            .iter()
            .find_map(|p| p.default_for_models.first().cloned())
            .or_else(|| rows.first().and_then(|p| p.models.first().cloned()))
            .unwrap_or_default();
        (Arc::new(router), default_model)
    };

    Ok(RuntimeContext::new(
        backend,
        Arc::new(Toolbox::new()),
        AgentConfig::new(default_model),
    ))
}
