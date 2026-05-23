//! Xiaoguai core binary — v0.5.1 wiring.
//!
//! Loads configuration, connects to Postgres + Valkey, applies migrations,
//! initializes JWT + RBAC + audit chain, then either runs the API server
//! (default) or executes a single subcommand (e.g. `smoke`).

mod audit_bridge;
mod today_bridge;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use xiaoguai_audit::{AuditEntry, ChainedAudit};
use xiaoguai_auth::{Authz, JwtValidator};
use xiaoguai_config::Settings;
use xiaoguai_storage::{cache::Cache, db};

#[derive(Parser, Debug)]
#[command(name = "xiaoguai-core", version, about = "Xiaoguai core binary")]
struct Cli {
    /// Path to a YAML config file. If absent, defaults + env are used.
    #[arg(long, env = "XIAOGUAI_CONFIG")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Connect to every component, perform a round-trip, exit 0 on success.
    Smoke,
    /// Run the long-lived server (placeholder until v0.5.5 wires axum routes).
    Serve,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,sqlx=warn")),
        )
        .init();

    let cli = Cli::parse();
    let settings = load_settings(cli.config.as_deref())?;

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        cfg.db = %settings.database.url,
        cfg.cache = %settings.cache.url,
        cfg.jwks = %settings.auth.jwks_url,
        "xiaoguai-core starting"
    );

    match cli.command.unwrap_or(Cmd::Serve) {
        Cmd::Smoke => run_smoke(&settings).await,
        Cmd::Serve => run_serve(&settings).await,
    }
}

fn load_settings(path: Option<&std::path::Path>) -> Result<Settings> {
    if let Some(p) = path {
        Settings::load_from_file(p)
            .map_err(|e| anyhow::anyhow!("config: {e}"))
            .with_context(|| format!("loading {}", p.display()))
    } else {
        Settings::load_from_env().map_err(|e| anyhow::anyhow!("config(env): {e}"))
    }
}

/// Boot every subsystem, do a round-trip on each, exit.
async fn run_smoke(settings: &Settings) -> Result<()> {
    tracing::info!("smoke: connecting to Postgres");
    let pool = db::connect(&settings.database.url, settings.database.max_connections)
        .await
        .context("pg connect")?;
    db::migrate(&pool).await.context("pg migrate")?;
    let row: (i32,) = sqlx::query_as("SELECT 1")
        .fetch_one(&pool)
        .await
        .context("pg select 1")?;
    anyhow::ensure!(row.0 == 1, "pg select 1 returned {}", row.0);
    tracing::info!("smoke: pg ok");

    tracing::info!("smoke: connecting to Valkey");
    let cache = Cache::connect(&settings.cache.url, settings.cache.key_prefix.clone())
        .await
        .context("valkey connect")?;
    let ts = chrono::Utc::now().to_rfc3339();
    cache
        .set(
            "smoke/heartbeat",
            &ts,
            Some(std::time::Duration::from_secs(60)),
        )
        .await
        .context("valkey set")?;
    let got: Option<String> = cache.get("smoke/heartbeat").await.context("valkey get")?;
    anyhow::ensure!(
        got.as_deref() == Some(ts.as_str()),
        "valkey round-trip mismatch"
    );
    tracing::info!("smoke: valkey ok");

    tracing::info!("smoke: initializing JWT validator (no network call)");
    let _jwt = JwtValidator::new(
        settings.auth.issuer.clone(),
        settings.auth.audience.clone(),
        settings.auth.jwks_url.clone(),
    );
    tracing::info!("smoke: jwt validator ok");

    tracing::info!("smoke: loading Casbin RBAC");
    let authz = Authz::new_default().await.context("rbac load")?;
    let allowed = authz
        .check("system_admin", "smoke-tenant", "/sessions/anything", "read")
        .await
        .context("rbac check")?;
    anyhow::ensure!(allowed, "system_admin should be allowed");
    tracing::info!("smoke: rbac ok");

    tracing::info!("smoke: audit chain round-trip");
    let chain = ChainedAudit::new(settings.audit.hmac_key.as_bytes());
    let prev = vec![0u8; 32];
    let entry = AuditEntry {
        ts: chrono::Utc::now(),
        tenant_id: "smoke-tenant".into(),
        actor: "system".into(),
        action: "smoke.run".into(),
        resource: None,
        details: serde_json::json!({"phase": "boot"}),
    };
    let hmac = chain.compute_hmac(&prev, &entry).context("audit hmac")?;
    anyhow::ensure!(hmac.len() == 32, "audit hmac length");
    tracing::info!("smoke: audit ok");

    tracing::info!("smoke: PASS");
    Ok(())
}

#[allow(clippy::too_many_lines, clippy::type_complexity)]
async fn run_serve(settings: &Settings) -> Result<()> {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use xiaoguai_agent::{AgentConfig, Toolbox};
    use xiaoguai_api::{
        audit::{AuditReader, AuditVerifier},
        AppState, CancelRegistry, RateLimiter,
    };
    use xiaoguai_audit::chain::sink::PgAuditSink;
    use xiaoguai_llm::{build_router, LlmBackend, MockBackend, OsEnvResolver};
    use xiaoguai_mcp::McpSupervisor;
    use xiaoguai_storage::repositories::{
        LlmProviderRepository, PgLlmProviderRepository, PgMcpServerRepository, PgMessageRepository,
        PgSessionRepository, PgTenantRepository,
    };

    use crate::audit_bridge::PgAuditAdapter;

    tracing::info!("serve: connecting to Postgres");
    let pool = db::connect(&settings.database.url, settings.database.max_connections)
        .await
        .context("pg connect")?;
    db::migrate(&pool).await.context("pg migrate")?;

    // v0.6.2: read system-wide LLM providers and assemble a router. The
    // resulting `LlmRouter` implements `LlmBackend`, so it drops in
    // wherever the old `MockBackend` used to live. If the registry is
    // empty we keep the `MockBackend` fallback so that fresh deployments
    // still boot and serve a deterministic response.
    let provider_repo = PgLlmProviderRepository::new(pool.clone());
    let rows = provider_repo
        .list_global()
        .await
        .context("pg list llm providers")?;
    let (backend, default_model): (Arc<dyn LlmBackend>, String) = if rows.is_empty() {
        tracing::warn!(
            "serve: llm_providers table is empty — falling back to MockBackend. \
             Use `xiaoguai provider register` to populate it."
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
            tracing::warn!(warning = %w, "serve: llm router build");
        }
        // Default agent model: prefer the first model that any provider
        // claims as a default; otherwise the first declared model on the
        // first provider; otherwise an empty string (caller must
        // override per-request).
        let default_model = rows
            .iter()
            .find_map(|p| p.default_for_models.first().cloned())
            .or_else(|| rows.first().and_then(|p| p.models.first().cloned()))
            .unwrap_or_default();
        tracing::info!(
            providers = rows.len(),
            default_model = %default_model,
            "serve: LlmRouter wired"
        );
        (Arc::new(router), default_model)
    };

    let auth = build_auth(settings);

    // v0.6.5: try to assemble the production audit bridge. The signing
    // key lives in the env var named by `settings.audit.signing_key_env`
    // — empty / missing means audit endpoints stay at 503 in production
    // rather than silently using `settings.audit.hmac_key` (which is the
    // dev-only fallback wired into `smoke`).
    let (audit_reader, audit_verifier): (
        Option<Arc<dyn AuditReader>>,
        Option<Arc<dyn AuditVerifier>>,
    ) = match std::env::var(&settings.audit.signing_key_env) {
        Ok(key) if !key.is_empty() => {
            let sink = Arc::new(PgAuditSink::new(pool.clone(), key.into_bytes()));
            let adapter = Arc::new(PgAuditAdapter::new(sink));
            tracing::info!(
                env = %settings.audit.signing_key_env,
                "serve: audit reader+verifier wired (PgAuditSink)"
            );
            (Some(adapter.clone()), Some(adapter))
        }
        _ => {
            tracing::warn!(
                env = %settings.audit.signing_key_env,
                "serve: audit signing key not set — /v1/admin/audit and /v1/admin/audit/verify will return 503"
            );
            (None, None)
        }
    };

    let mcp_servers_repo: Arc<dyn xiaoguai_storage::repositories::McpServerRepository> =
        Arc::new(PgMcpServerRepository::new(pool.clone()));

    let state = AppState {
        sessions: Arc::new(PgSessionRepository::new(pool.clone())),
        messages: Arc::new(PgMessageRepository::new(pool.clone())),
        backend,
        toolbox: Arc::new(Toolbox::new()),
        agent_defaults: AgentConfig::new(default_model.clone()),
        cancels: Arc::new(CancelRegistry::new()),
        mcp_servers: Some(mcp_servers_repo),
        auth,
        authz: build_authz(settings).await.context("build authz")?,
        tenants: Some(Arc::new(PgTenantRepository::new(pool.clone()))),
        // v0.6.3 default: sustain 20 req/s with a 40 token burst per
        // tenant. Production should tune via config; the knob isn't
        // exposed yet.
        rate_limiter: Some(Arc::new(RateLimiter::new(20.0, 40.0))),
        audit: audit_reader,
        audit_verifier,
        // v0.9.1: opt-in publishing of the Toolbox as an MCP server at
        // /v1/mcp/serve. Controlled by env var `XIAOGUAI_MCP__PUBLISH`
        // — only flip in deployments that *want* external agents to
        // call into us.
        mcp_publish_enabled: std::env::var("XIAOGUAI_MCP__PUBLISH")
            .map(|s| matches!(s.as_str(), "1" | "true" | "yes"))
            .unwrap_or(false),
        // v0.9.4.1: live supervisor so marketplace installs spawn the
        // new server immediately.
        mcp_supervisor: Some(Arc::new(McpSupervisor::new())),
        // v0.11.1: audit-first console substrate. The PG adapter walks
        // sessions / im_conversations / scheduled_job_runs and merges
        // them client-side.
        today: Some(crate::today_bridge::PgTodayReader::arc(pool.clone())),
    };

    // v0.7.4: mount the Feishu webhook with a PG-backed history store by
    // default (multi-replica safe). Operators can fall back to the
    // single-replica in-process store by setting
    // `XIAOGUAI_IM__USE_IN_PROCESS_HISTORY=true`.
    let im_router = build_feishu_gateway(settings, &pool, &state, &default_model);

    let addr: SocketAddr = format!("{}:{}", settings.server.host, settings.server.port)
        .parse()
        .with_context(|| {
            format!(
                "parse bind addr {}:{}",
                settings.server.host, settings.server.port
            )
        })?;
    let (local, fut) = serve_with_state_and_extras(addr, state, im_router)
        .await
        .context("bind api")?;
    tracing::info!(%local, "serve: api listening");

    tokio::select! {
        res = fut => res.context("axum serve")?,
        _ = tokio::signal::ctrl_c() => tracing::info!("serve: shutdown via ctrl-c"),
    }
    Ok(())
}

/// v0.7.4: assemble the Feishu IM gateway router. Returns `None` when
/// the operator hasn't supplied a Feishu signing key
/// (`XIAOGUAI_IM_FEISHU__VERIFICATION_TOKEN`); mounting the route with
/// an empty signing key would accept every payload.
fn build_feishu_gateway(
    settings: &xiaoguai_config::Settings,
    pool: &sqlx::PgPool,
    state: &xiaoguai_api::AppState,
    default_model: &str,
) -> Option<axum::Router> {
    use std::sync::Arc;
    use xiaoguai_im_feishu::FeishuProvider;
    use xiaoguai_im_gateway::{
        mount_feishu_with_history, ConversationHistory, ImHistoryStore, ImProvider,
        PgImHistoryStore,
    };
    use xiaoguai_storage::repositories::PgImIdentityRepository;

    let signing_key = std::env::var("XIAOGUAI_IM_FEISHU__VERIFICATION_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())?;

    let history: Arc<dyn ImHistoryStore> = if settings.im.use_in_process_history {
        tracing::info!(
            "serve: IM history using in-process ConversationHistory (XIAOGUAI_IM__USE_IN_PROCESS_HISTORY=true)"
        );
        Arc::new(ConversationHistory::new(
            settings.im.max_messages_per_conversation,
        ))
    } else {
        tracing::info!(
            cap = settings.im.max_messages_per_conversation,
            "serve: IM history using PgImHistoryStore"
        );
        Arc::new(PgImHistoryStore::new(
            Arc::new(PgImIdentityRepository::new(pool.clone())),
            state.sessions.clone(),
            state.messages.clone(),
            default_model.to_string(),
            settings.im.max_messages_per_conversation,
        ))
    };
    let provider: Arc<dyn ImProvider> = match (
        std::env::var("XIAOGUAI_IM_FEISHU__APP_ID").ok(),
        std::env::var("XIAOGUAI_IM_FEISHU__APP_SECRET").ok(),
    ) {
        (Some(app_id), Some(app_secret)) if !app_id.is_empty() && !app_secret.is_empty() => {
            match xiaoguai_im_feishu::HttpFeishuClient::new() {
                Ok(client) => Arc::new(FeishuProvider::with_api_sink(
                    signing_key,
                    Arc::new(client),
                    app_id,
                    app_secret,
                )),
                Err(e) => {
                    tracing::error!(error = %e, "serve: HttpFeishuClient build failed — falling back to stub reply sink");
                    Arc::new(FeishuProvider::new(signing_key))
                }
            }
        }
        _ => {
            tracing::warn!(
                "serve: XIAOGUAI_IM_FEISHU__APP_ID / __APP_SECRET unset — Feishu replies will be stubbed"
            );
            Arc::new(FeishuProvider::new(signing_key))
        }
    };
    Some(mount_feishu_with_history(state.clone(), provider, history))
}

/// Bind the main API router and merge the optional IM gateway router on
/// top. Equivalent to `serve_with_state` but accepts a second router
/// fragment so we can compose IM webhooks without adding a public API
/// surface for it (the IM mount is per-operator wiring, not part of the
/// stable HTTP contract).
async fn serve_with_state_and_extras(
    addr: std::net::SocketAddr,
    state: xiaoguai_api::AppState,
    extra: Option<axum::Router>,
) -> Result<(
    std::net::SocketAddr,
    impl std::future::Future<Output = std::io::Result<()>>,
)> {
    use tokio::net::TcpListener;

    let mut app = xiaoguai_api::router(state);
    if let Some(r) = extra {
        app = app.merge(r);
    }
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    let local = listener.local_addr().context("read local addr")?;
    let fut = async move { axum::serve(listener, app.into_make_service()).await };
    Ok((local, fut))
}

/// Wire the JWT validator when `auth.required` is on. Returns `None` to
/// keep the dev-mode "body-supplied identity" path; returns
/// `Some(JwtTokenValidator)` to require Bearer tokens on `/v1/**`.
fn build_auth(
    settings: &Settings,
) -> Option<std::sync::Arc<dyn xiaoguai_api::auth::TokenValidator>> {
    use std::sync::Arc;
    use xiaoguai_api::auth::{JwtTokenValidator, TokenValidator};
    if !settings.auth.required {
        return None;
    }
    let validator = xiaoguai_auth::JwtValidator::new(
        settings.auth.issuer.clone(),
        settings.auth.audience.clone(),
        settings.auth.jwks_url.clone(),
    );
    let arc_validator = Arc::new(validator);
    let wrapper: Arc<dyn TokenValidator> = Arc::new(JwtTokenValidator(arc_validator));
    tracing::info!(
        issuer = %settings.auth.issuer,
        audience = %settings.auth.audience,
        "serve: JWT validator enabled (auth.required=true)"
    );
    Some(wrapper)
}

/// Load the Casbin policy and return an `Authz`. When auth is disabled
/// we still build it — the route middleware checks `auth.required` to
/// decide whether to enforce. This keeps boot deterministic: if the
/// shipped policy file fails to parse, the binary fails fast.
async fn build_authz(settings: &Settings) -> Result<Option<std::sync::Arc<xiaoguai_auth::Authz>>> {
    use std::sync::Arc;
    if !settings.auth.required {
        return Ok(None);
    }
    let authz = xiaoguai_auth::Authz::new_default()
        .await
        .context("load casbin policy")?;
    tracing::info!("serve: Casbin authz loaded");
    Ok(Some(Arc::new(authz)))
}
