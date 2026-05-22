//! Xiaoguai core binary — v0.5.1 wiring.
//!
//! Loads configuration, connects to Postgres + Valkey, applies migrations,
//! initializes JWT + RBAC + audit chain, then either runs the API server
//! (default) or executes a single subcommand (e.g. `smoke`).

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

async fn run_serve(settings: &Settings) -> Result<()> {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use xiaoguai_agent::{AgentConfig, Toolbox};
    use xiaoguai_api::{serve_with_state, AppState, CancelRegistry};
    use xiaoguai_llm::{LlmBackend, MockBackend};
    use xiaoguai_storage::repositories::{PgMessageRepository, PgSessionRepository};

    tracing::info!("serve: connecting to Postgres");
    let pool = db::connect(&settings.database.url, settings.database.max_connections)
        .await
        .context("pg connect")?;
    db::migrate(&pool).await.context("pg migrate")?;

    // v0.5.6 wires the minimum useful AppState. Real LLM provider routing
    // (LlmRouter pulling from `llm_providers` repo) lands in v0.6 with the
    // auth + tenant-context plumbing.
    let backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_response(
        "MockBackend is the v0.5.6 default. Configure a real backend in v0.6.",
    ));
    let state = AppState {
        sessions: Arc::new(PgSessionRepository::new(pool.clone())),
        messages: Arc::new(PgMessageRepository::new(pool)),
        backend,
        toolbox: Arc::new(Toolbox::new()),
        agent_defaults: AgentConfig::new("mock"),
        cancels: Arc::new(CancelRegistry::new()),
    };

    let addr: SocketAddr = format!("{}:{}", settings.server.host, settings.server.port)
        .parse()
        .with_context(|| {
            format!(
                "parse bind addr {}:{}",
                settings.server.host, settings.server.port
            )
        })?;
    let (local, fut) = serve_with_state(addr, state).await.context("bind api")?;
    tracing::info!(%local, "serve: api listening");

    tokio::select! {
        res = fut => res.context("axum serve")?,
        _ = tokio::signal::ctrl_c() => tracing::info!("serve: shutdown via ctrl-c"),
    }
    Ok(())
}
