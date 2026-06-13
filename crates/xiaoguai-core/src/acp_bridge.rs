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
use xiaoguai_audit::chain::sink::SqliteAuditSink;
use xiaoguai_config::Settings;
use xiaoguai_llm::{build_router, LlmBackend, MockBackend, OsEnvResolver};
use xiaoguai_runtime::RuntimeContext;
use xiaoguai_storage::db;
use xiaoguai_storage::repositories::{LlmProviderRepository, SqliteLlmProviderRepository};

use crate::coding_bridge::{build_coding_toolbox, coding_allow_egress, coding_workspace_root};

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

/// Assemble the runtime context: LLM backend (router or mock) + the toolbox +
/// agent defaults. Mirrors `run_serve`'s backend selection AND its opt-in
/// coding-tool registration, so an IDE turn over ACP can edit/commit/rollback
/// (HotL-gated + audited) — not just chat.
async fn build_runtime_context(settings: &Settings) -> Result<RuntimeContext> {
    let pool = db::connect(&settings.database.url, settings.database.max_connections)
        .await
        .context("acp: db connect")?;
    db::migrate(&pool).await.context("acp: db migrate")?;

    let provider_repo = SqliteLlmProviderRepository::from_env(pool.clone())
        .context("acp: load at-rest encryption key")?;
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

    // Opt-in coding tools, same gate as `run_serve`: a workspace must be set
    // (XIAOGUAI_CODING_WORKSPACE) AND an audit signing key configured — no
    // ungoverned coding. Egress (git_push/open_pr) needs the further
    // XIAOGUAI_CODING_ALLOW_EGRESS opt-in.
    let toolbox = match (signing_key(settings), coding_workspace_root()) {
        (Some(key), Some(root)) => {
            let sink = Arc::new(SqliteAuditSink::new(pool.clone(), key));
            let allow_egress = coding_allow_egress();
            match build_coding_toolbox(sink, &root, allow_egress).await {
                Ok(tb) => {
                    tracing::info!(
                        workspace = %root.display(),
                        tools = tb.len(),
                        egress = allow_egress,
                        "acp: governed coding tools registered into the agent toolbox"
                    );
                    Arc::new(tb)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "acp: failed to build coding toolbox — chat-only");
                    Arc::new(Toolbox::new())
                }
            }
        }
        _ => Arc::new(Toolbox::new()),
    };

    Ok(RuntimeContext::new(
        backend,
        toolbox,
        AgentConfig::new(default_model),
    ))
}

/// The audit signing key from the configured env var, or `None` when unset —
/// the same source `run_serve` uses to decide whether governed coding is wired.
fn signing_key(settings: &Settings) -> Option<Vec<u8>> {
    std::env::var(&settings.audit.signing_key_env)
        .ok()
        .filter(|k| !k.is_empty())
        .map(String::into_bytes)
}
