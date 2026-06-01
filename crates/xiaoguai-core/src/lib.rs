//! Xiaoguai core — library + binary wiring.
//!
//! Loads configuration, connects to Postgres + Valkey, applies migrations,
//! initializes JWT + RBAC + audit chain, then either runs the API server
//! (default) or executes a single subcommand (e.g. `smoke`).
//!
//! The boot flow lives in [`run_with_cli`] so both the legacy `xiaoguai-core`
//! binary and the unified `xiaoguai serve` subcommand can drive it.
//!
//! ## Tier-1b: in-process cache fallback
//!
//! When `cache.url` is empty (or any non-`redis://` URL) the storage layer
//! boots an in-process `DashMap` instead of opening a Redis/Valkey
//! connection. This makes the single-binary, air-gapped path viable: a
//! single-tenant deploy can run with just Postgres + xiaoguai, no Valkey
//! sidecar. The boot log emits a distinct `tracing::info!` line so operators
//! can confirm at startup which backend is live. See
//! `docs/runbooks/cache-fallback.md`.

#![forbid(unsafe_code)]
//!
//! ## sd-notify integration (v1.1.6.2)
//!
//! When running under systemd with `Type=notify` the binary sends:
//! - `READY=1` once all listeners are bound and subsystems started.
//! - `WATCHDOG=1` pings on a background interval when `WATCHDOG_USEC` is
//!   set by systemd (opt-in via `WatchdogSec=` in the unit file).
//! - `STOPPING=1` before the graceful-shutdown path.
//!
//! All sd-notify calls are gated on `#[cfg(target_os = "linux")]` so
//! macOS and Windows developer builds compile and run without change.

mod audit_bridge;
mod eval_bridge;
pub mod hotl_bridge;
mod memory_bridge;
pub mod outcomes_bridge;
#[cfg(feature = "packs")]
pub mod packs;
mod scheduler_bridge;
mod sd_notify_bridge;
mod sessions_bridge;
pub mod skill_author_bridge;
pub mod skills_bridge;
mod today_bridge;
mod usage_bridge;
pub mod workspace_bridge;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use xiaoguai_audit::{AuditEntry, ChainedAudit};
use xiaoguai_auth::{Authz, JwtValidator};
use xiaoguai_config::Settings;
use xiaoguai_storage::{cache::Cache, db, ReadWritePool};

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

/// Run the binary entry point: parse CLI args, init tracing, load settings,
/// and dispatch to either [`run_smoke`] or [`run_serve`].
///
/// The `xiaoguai-core` binary and the `xiaoguai serve` subcommand both call
/// this. It assumes a tokio runtime is already running (the calling binary
/// supplies it via `#[tokio::main]`).
pub async fn run_with_cli() -> Result<()> {
    // Best-effort: skip re-init if the caller (e.g. tests) already set a
    // subscriber. `try_init` returns Err if a subscriber is already global.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,sqlx=warn")),
        )
        .try_init();

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

pub fn load_settings(path: Option<&std::path::Path>) -> Result<Settings> {
    if let Some(p) = path {
        Settings::load_from_file(p)
            .map_err(|e| anyhow::anyhow!("config: {e}"))
            .with_context(|| format!("loading {}", p.display()))
    } else {
        Settings::load_from_env().map_err(|e| anyhow::anyhow!("config(env): {e}"))
    }
}

/// Boot every subsystem, do a round-trip on each, exit.
pub async fn run_smoke(settings: &Settings) -> Result<()> {
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

    tracing::info!("smoke: connecting to cache");
    let cache = Cache::connect(&settings.cache.url, settings.cache.key_prefix.clone())
        .await
        .context("cache connect")?;
    if cache.is_in_process() {
        // The in-process backend is a process-local DashMap; a set/get
        // round-trip would only prove that DashMap works. Skip the trip and
        // log the mode so operators see at startup which path is live.
        tracing::info!("smoke: cache: in-process (no round-trip)");
    } else {
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
    }

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
pub async fn run_serve(settings: &Settings) -> Result<()> {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use xiaoguai_agent::{AgentConfig, Toolbox};
    use xiaoguai_api::{
        audit::{AuditReader, AuditVerifier},
        AppState, CancelRegistry, RateClass, RateLimitState,
    };
    use xiaoguai_audit::chain::sink::PgAuditSink;
    use xiaoguai_llm::{build_router, LlmBackend, MockBackend, OsEnvResolver};
    use xiaoguai_mcp::McpSupervisor;
    #[cfg(feature = "observability")]
    use xiaoguai_observability;
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

    // Sprint-8 S8-5: refuse-to-start when MCP OAuth tokens exist but the
    // encryption keyring is unavailable. Fresh-install path (empty table)
    // boots without the env var.
    check_mcp_oauth_keyring(&pool).await?;

    // v1.1.4.1: build the read/write pool router.
    // `DATABASE_REPLICA_URLS` (comma-separated) — optional; defaults to
    // primary-only when absent, preserving v1.1.4 behaviour exactly.
    let rw_pool = {
        let replicas = ReadWritePool::replicas_from_env(settings.database.max_connections)
            .await
            .context("replica pool connect")?;
        ReadWritePool::new(pool.clone(), replicas)
    };

    // v0.6.2: read system-wide LLM providers and assemble a router. The
    // resulting `LlmRouter` implements `LlmBackend`, so it drops in
    // wherever the old `MockBackend` used to live. If the registry is
    // empty we keep the `MockBackend` fallback so that fresh deployments
    // still boot and serve a deterministic response.
    let provider_repo = PgLlmProviderRepository::new(pool.clone());
    let mut rows = provider_repo
        .list_global()
        .await
        .context("pg list llm providers")?;
    // Local-first: OLLAMA_HOST repoints the seeded `ollama-local` provider at a
    // different endpoint (e.g. a dedicated GPU box) without a SQL change. Uses
    // the standard Ollama env var so it matches operator expectations.
    if let Ok(host) = std::env::var("OLLAMA_HOST") {
        let host = host.trim();
        if !host.is_empty() {
            for r in rows.iter_mut().filter(|r| r.id.as_str() == "ollama-local") {
                tracing::info!(
                    endpoint = %host,
                    "serve: OLLAMA_HOST override applied to ollama-local"
                );
                r.endpoint = host.to_string();
            }
        }
    }
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
    // Sprint-8 S8-7: hoist the PgAuditSink so the skill_author_bridge can
    // reuse the same signing chain. `None` here keeps skill-author audit
    // wiring off when the signing key env var is empty.
    let pg_audit_sink: Option<Arc<PgAuditSink>> =
        match std::env::var(&settings.audit.signing_key_env) {
            Ok(key) if !key.is_empty() => {
                Some(Arc::new(PgAuditSink::new(pool.clone(), key.into_bytes())))
            }
            _ => None,
        };

    let (audit_reader, audit_verifier, audit_chain_exporter): (
        Option<Arc<dyn AuditReader>>,
        Option<Arc<dyn AuditVerifier>>,
        Option<Arc<dyn xiaoguai_api::audit::AuditChainExporter>>,
    ) = if let Some(sink) = &pg_audit_sink {
        let adapter = Arc::new(PgAuditAdapter::new(sink.clone()));
        tracing::info!(
            env = %settings.audit.signing_key_env,
            "serve: audit reader+verifier+exporter wired (PgAuditSink)"
        );
        (
            Some(adapter.clone() as Arc<dyn AuditReader>),
            Some(adapter.clone() as Arc<dyn AuditVerifier>),
            Some(adapter as Arc<dyn xiaoguai_api::audit::AuditChainExporter>),
        )
    } else {
        tracing::warn!(
            env = %settings.audit.signing_key_env,
            "serve: audit signing key not set — /v1/admin/audit, /v1/admin/audit/verify, and /v1/audit/exports will return 503"
        );
        (None, None, None)
    };

    let mcp_servers_repo: Arc<dyn xiaoguai_storage::repositories::McpServerRepository> =
        Arc::new(PgMcpServerRepository::new(pool.clone()));

    // v0.12.0: scheduler bootstrap. Off by default so existing
    // deployments don't change behaviour. When enabled we spawn a
    // `JobRunner::run_loop` on a tokio task, wire the PG repositories,
    // run the agent loop through `RuntimeJobExecutor`, and hand the
    // `WebhookSource` to `AppState` so `/v1/admin/scheduler/webhooks/...`
    // can fire reactive jobs.
    //
    // v0.12.1: also wire the `PgScheduledSessionWriter` into the
    // executor (so `scheduled_job_runs.session_id` populates and the
    // audit-first console can drill into transcripts) and the
    // `PgScheduledJobUpserter` into AppState for `POST /v1/admin/scheduler/jobs`.
    let toolbox = Arc::new(Toolbox::new());

    // Tier-2 prereq: build the HOTL enforcer once, share between
    // `AppState.hotl_enforcer` (gating LLM calls upstream in
    // `send_message`) and `agent_defaults.hotl_gate` (gating each tool
    // dispatch inside the ReAct loop). The enforcer is fail-closed on PG
    // errors; `EnforcerGate` maps that into a per-tool `Deny` verdict.
    //
    // Sharing one PG-backed enforcer means the budget counter is unified:
    // a tenant that's burned its `tool_call.*` budget can still call the
    // LLM (different scope), and vice versa.
    let hotl_policy_store_pg = Arc::new(crate::hotl_bridge::PgHotlPolicyStore::new(pool.clone()));
    let hotl_enforcer_arc: Arc<dyn xiaoguai_api::hotl::enforcer::HotlEnforcer> = Arc::new(
        crate::hotl_bridge::PgHotlEnforcer::new(pool.clone(), hotl_policy_store_pg.clone()),
    );
    // Sprint-12 S12-4 / Sprint-13 S13-5: the `DecisionRegistry` is
    // constructed ONCE here and shared between the gate adapter (so
    // `SuspendingHotlGate::check` can mint tickets against it) and
    // `AppState.decision_registry` (so `POST /v1/hotl/decisions` can
    // resolve waiters on it). A second registry would silently no-op
    // resolves and hang the loop until the 24h default expiry.
    //
    // Sprint-13 S13-5: the registry is wired to
    // `PgHotlEscalationRepository` and uses `replay_from_storage` so any
    // `hotl_pending` rows that survived a restart are reattached BEFORE
    // the HTTP server starts accepting requests. The replay log line
    // `hotl: replayed N pending decision waiters from PG` is the SRE
    // signal that the boot recovery path actually ran.
    let hotl_escalation_store: std::sync::Arc<
        dyn xiaoguai_storage::repositories::HotlEscalationStore,
    > = std::sync::Arc::new(
        xiaoguai_storage::repositories::PgHotlEscalationRepository::new(pool.clone()),
    );
    let decision_registry =
        xiaoguai_api::hotl::decision_registry::DecisionRegistry::replay_from_storage(
            hotl_escalation_store.clone(),
            chrono::Utc::now(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("HOTL decision registry boot replay failed: {e}"))?;
    // Per design (`lld-agent.md` §4.5): default suspend window is 24h.
    // Sprint-13 S13-7: per-scope-class overrides land via
    // `agent.hotl.expiry` (S13-0 surface) — empty map preserves the
    // single-knob v1.9.x behaviour byte-for-byte.
    let hotl_default_expiry = std::time::Duration::from_secs(24 * 3600);
    // Sprint-13 S13-6: wire the `PgHotlRedactionRepo` + per-tenant
    // policy required flag + audit sink into the suspend gate so
    // operator banners see masked tool args and the audit chain carries
    // the matched policy id.
    let hotl_redaction_repo: Arc<
        dyn xiaoguai_storage::repositories::hotl_redaction::HotlRedactionRepo,
    > = Arc::new(
        xiaoguai_storage::repositories::hotl_redaction::PgHotlRedactionRepo::new(pool.clone()),
    );
    let hotl_gate_audit_sink: Option<Arc<dyn xiaoguai_api::hotl::audit::HotlAuditSink>> =
        pg_audit_sink
            .as_ref()
            .map(|sink| crate::hotl_bridge::PgHotlAuditSink::arc(sink.clone()));
    let hotl_gate: Arc<dyn xiaoguai_agent::HotlGate> =
        crate::hotl_bridge::build_hotl_gate_with_redaction(
            settings.agent.hotl.suspend_on_escalate,
            hotl_enforcer_arc.clone(),
            decision_registry.clone(),
            hotl_default_expiry,
            settings.agent.hotl.expiry.clone(),
            hotl_redaction_repo,
            settings.agent.hotl.redaction_policy_required,
            hotl_gate_audit_sink,
        );
    tracing::info!(
        suspend_on_escalate = settings.agent.hotl.suspend_on_escalate,
        redaction_policy_required = settings.agent.hotl.redaction_policy_required,
        "serve: HOTL gate selected per agent.hotl.suspend_on_escalate (+ redaction wiring)"
    );

    let agent_defaults = AgentConfig::new(default_model.clone()).with_hotl_gate(hotl_gate.clone());
    tracing::info!("serve: agent ReAct loop wired with HOTL gate (scope = tool_call.<name>)");
    // Build these once so both the executor session writer and the
    // upserter on AppState see the same PG handles.
    let pg_session_repo: Arc<dyn xiaoguai_storage::repositories::SessionRepository> =
        Arc::new(PgSessionRepository::new(pool.clone()));
    let pg_message_repo: Arc<dyn xiaoguai_storage::repositories::MessageRepository> =
        Arc::new(PgMessageRepository::new(pool.clone()));
    // v0.12.x.1: also wire `PgScheduledJobsReader` (admin-ui Scheduler
    // pane backend) and the per-tenant webhook token validator + admin
    // (out-of-band webhook auth). All three are `None` when scheduler is
    // disabled — the matching routes return 503.
    let (
        scheduler_handle,
        webhook_pusher,
        job_upserter,
        scheduler_jobs_reader,
        webhook_token_validator,
        webhook_token_admin,
    ): (
        Option<tokio::task::JoinHandle<Result<(), xiaoguai_scheduler::RunnerError>>>,
        Option<Arc<dyn xiaoguai_api::scheduler::WebhookPusher>>,
        Option<Arc<dyn xiaoguai_api::scheduler::ScheduledJobUpserter>>,
        Option<Arc<dyn xiaoguai_api::scheduler::ScheduledJobsReader>>,
        Option<Arc<dyn xiaoguai_api::scheduler::WebhookTokenValidator>>,
        Option<Arc<dyn xiaoguai_api::scheduler::WebhookTokenAdmin>>,
    ) = if settings.scheduler.enabled {
        let runtime_ctx = crate::scheduler_bridge::build_runtime_ctx(
            backend.clone(),
            toolbox.clone(),
            agent_defaults.clone(),
        );
        let session_writer: Arc<dyn xiaoguai_scheduler::ScheduledSessionWriter> =
            Arc::new(crate::scheduler_bridge::PgScheduledSessionWriter::new(
                pg_session_repo.clone(),
                pg_message_repo.clone(),
            ));
        // v0.12.x.1: CompositeExecutor dispatches by `payload.kind`.
        // Default = RuntimeJobExecutor (every existing job; payload has
        // no `kind`). Registered = RagReindexExecutor for
        // `kind == "rag_reindex"`. The default arm preserves v0.12.0
        // behaviour exactly — only payloads that opt in via `kind` see
        // the alternate dispatch.
        let runtime_executor: Arc<dyn xiaoguai_scheduler::JobExecutor> = Arc::new(
            xiaoguai_scheduler::RuntimeJobExecutor::new(runtime_ctx)
                .with_session_writer(session_writer),
        );
        let rag_client: Arc<dyn xiaoguai_rag::RagClient> =
            Arc::new(xiaoguai_rag::InMemoryRagClient::new());
        let rag_executor: Arc<dyn xiaoguai_scheduler::JobExecutor> =
            Arc::new(crate::scheduler_bridge::RagReindexExecutor::new(rag_client));
        let executor: Arc<dyn xiaoguai_scheduler::JobExecutor> = Arc::new(
            xiaoguai_scheduler::CompositeExecutor::new(runtime_executor)
                .register("rag_reindex", rag_executor),
        );
        let pg_jobs = Arc::new(xiaoguai_scheduler::PgJobRepository::new(pool.clone()));
        let jobs: Arc<dyn xiaoguai_scheduler::JobRepository> = pg_jobs.clone();
        let runs: Arc<dyn xiaoguai_scheduler::JobRunRepository> =
            Arc::new(xiaoguai_scheduler::PgJobRunRepository::new(pool.clone()));
        // Audit appender: route through the same PgAuditSink the audit
        // bridge already constructed when the signing key was present.
        // When audit is unwired we fall back to NullAuditAppender so the
        // scheduler still runs (audit gap is logged at startup).
        let audit_appender: Arc<dyn xiaoguai_scheduler::AuditAppender> = match std::env::var(
            &settings.audit.signing_key_env,
        ) {
            Ok(key) if !key.is_empty() => {
                let mut sink = PgAuditSink::new(pool.clone(), key.into_bytes());
                if audit_redaction_enabled() {
                    sink = sink.with_redactor(xiaoguai_audit::Redactor::new());
                    tracing::info!(
                        "serve: audit PII redaction ENABLED (XIAOGUAI_AUDIT_REDACT_PII)"
                    );
                } else {
                    tracing::warn!(
                        "serve: audit PII redaction DISABLED via XIAOGUAI_AUDIT_REDACT_PII"
                    );
                }
                let sink = Arc::new(sink);
                Arc::new(crate::scheduler_bridge::PgSchedulerAuditAppender::new(sink))
            }
            _ => {
                tracing::warn!("serve: scheduler audit appender = NullAuditAppender (no signing key); scheduler runs will NOT enter the audit chain");
                Arc::new(xiaoguai_scheduler::NullAuditAppender)
            }
        };
        let webhook_source = Arc::new(xiaoguai_scheduler::WebhookSource::new());
        let (event_tx, event_rx) = xiaoguai_scheduler::event_channel();
        if let Err(e) =
            xiaoguai_scheduler::TriggerSource::start(webhook_source.as_ref(), event_tx.clone())
                .await
        {
            anyhow::bail!("scheduler webhook source start: {e}");
        }

        // v0.12.2: optional FileWatchSource sharing the same event
        // channel. Routes come from two places: the static
        // `[scheduler.file_watch].routes` block in config (ops-friendly,
        // no DB write needed) and the persisted `scheduled_jobs` rows
        // whose `trigger.type == "file_watch"` (operator-friendly, the
        // admin pane creates them). Errors here are logged but do NOT
        // bail the server — a misconfigured watched path shouldn't kill
        // every other scheduler capability.
        if settings.scheduler.file_watch.enabled {
            if let Err(e) = crate::scheduler_bridge::spawn_file_watch_source(
                &settings.scheduler.file_watch,
                jobs.as_ref(),
                event_tx,
            )
            .await
            {
                tracing::error!(error = %e, "serve: file_watch source bootstrap failed; continuing without it");
            }
        } else {
            tracing::info!(
                "serve: file_watch source disabled (set [scheduler.file_watch].enabled = true to opt in)"
            );
        }

        let runner = xiaoguai_scheduler::JobRunner::new(jobs, runs, executor, audit_appender)
            .with_options(xiaoguai_scheduler::RunnerOptions {
                max_jobs_per_tick: 32,
                max_retry_sleep_secs: 30,
                budget_limit_per_user_per_day: xiaoguai_scheduler::DEFAULT_PROACTIVE_BUDGET_PER_DAY,
            });
        let runner = Arc::new(runner);
        let tick = std::time::Duration::from_secs(settings.scheduler.tick_interval_secs);
        let runner_for_task = runner.clone();
        let handle =
            tokio::spawn(async move { runner_for_task.run_loop(event_rx, Some(tick)).await });
        tracing::info!(
            tick_secs = settings.scheduler.tick_interval_secs,
            "serve: scheduler JobRunner spawned"
        );
        let pusher: Arc<dyn xiaoguai_api::scheduler::WebhookPusher> = Arc::new(
            crate::scheduler_bridge::WebhookSourceAdapter::new(webhook_source.clone()),
        );
        let upserter: Arc<dyn xiaoguai_api::scheduler::ScheduledJobUpserter> = Arc::new(
            crate::scheduler_bridge::PgScheduledJobUpserter::new(pg_jobs.clone()),
        );
        // v0.12.x.1: admin-ui Scheduler pane reader + "Run now" handle.
        let jobs_reader: Arc<dyn xiaoguai_api::scheduler::ScheduledJobsReader> = Arc::new(
            crate::scheduler_bridge::PgScheduledJobsReader::new(pg_jobs, runner.clone()),
        );
        // v0.12.x.1: per-tenant webhook tokens — PG validator + admin.
        let token_validator: Arc<dyn xiaoguai_api::scheduler::WebhookTokenValidator> = Arc::new(
            crate::scheduler_bridge::PgWebhookTokenValidator::new(pool.clone()),
        );
        let token_admin: Arc<dyn xiaoguai_api::scheduler::WebhookTokenAdmin> = Arc::new(
            crate::scheduler_bridge::PgWebhookTokenAdmin::new(pool.clone()),
        );
        (
            Some(handle),
            Some(pusher),
            Some(upserter),
            Some(jobs_reader),
            Some(token_validator),
            Some(token_admin),
        )
    } else {
        tracing::info!("serve: scheduler disabled (set [scheduler].enabled = true to opt in)");
        (None, None, None, None, None, None)
    };

    // v0.12.1: NL → ScheduledJob compiler. Always wire when we have an
    // LlmBackend (which is always — Mock fallback included). Independent
    // of the scheduler-enabled flag: an operator can use compile to
    // preview suggestions even before flipping the runner on. Upsert
    // still requires the scheduler to be enabled, though, so the
    // compile-then-save flow only completes when both are on.
    let nl_job_compiler: Option<Arc<dyn xiaoguai_api::scheduler::NlJobCompiler>> = Some(Arc::new(
        crate::scheduler_bridge::LlmNlJobCompiler::new(backend.clone(), default_model.clone()),
    ));
    tracing::info!(
        model = %default_model,
        "serve: NlJobCompiler wired"
    );

    let state = AppState {
        sessions: pg_session_repo.clone(),
        messages: pg_message_repo.clone(),
        backend,
        toolbox: toolbox.clone(),
        agent_defaults: agent_defaults.clone(),
        cancels: Arc::new(CancelRegistry::new()),
        mcp_servers: Some(mcp_servers_repo),
        auth,
        authz: build_authz(settings, &pool).await.context("build authz")?,
        tenants: Some(Arc::new(PgTenantRepository::new(pool.clone()))),
        // v0.6.3 / v1.2.20: per-tenant rate limiting. Legacy single-class
        // limiter is superseded by rate_limit_state (set at end of struct).
        rate_limiter: None,
        audit: audit_reader,
        audit_verifier,
        audit_chain_exporter,
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
        today: Some(crate::today_bridge::PgTodayReader::arc(rw_pool.clone())),
        // v0.11.2: eval pane. The PG case-from-session source feeds
        // operator "convert prod run to regression case" requests.
        eval: Some(build_eval_service(settings, pool.clone())),
        // v0.12.0: webhook → JobRunner adapter. None when scheduler is
        // disabled — the route then returns 503.
        webhook_pusher,
        // v0.12.1: NL → ScheduledJob compiler (always wired) +
        // ScheduledJob upserter (only when scheduler is enabled).
        nl_job_compiler,
        job_upserter,
        // v1.1.2: conversation fork — always wired in production
        // since it only needs the SessionRepository the rest of the
        // binary already holds.
        session_forker: Some(crate::sessions_bridge::PgSessionForker::arc(
            pg_session_repo.clone(),
        )),
        // v1.1.1: token-usage aggregator backing /v1/usage and the
        // admin-ui Usage pane (plus the Today pane's 24h summary card).
        // Always wired in production — the underlying token_usage table
        // is unconditional (migration 0004).
        usage_reader: Some(crate::usage_bridge::PgUsageReader::arc(rw_pool.clone())),
        // v0.12.x.1: per-tenant webhook token validator + admin CRUD
        // + admin-ui Scheduler pane jobs reader. All `None` when the
        // scheduler is disabled — the matching routes return 503.
        webhook_token_validator,
        webhook_token_admin,
        scheduler_jobs_reader,
        rate_limit_state: Some(RateLimitState::in_memory(RateClass::Standard)),
        // v1.2.3: HOTL boundary policy — PgHotlPolicyStore + PgHotlEnforcer
        // wired here (migration 0011 provides both tables). One store +
        // one enforcer is shared between the CRUD handle, the
        // `send_message` LLM-call gate (api crate), and the in-loop
        // per-tool gate (agent crate, threaded via `agent_defaults`).
        hotl_policy_store: Some(
            hotl_policy_store_pg.clone() as Arc<dyn xiaoguai_api::hotl::policy::HotlPolicyStore>
        ),
        hotl_enforcer: Some(hotl_enforcer_arc.clone()),
        // Sprint-12 S12-7: PG impls of HotlDecisionStore + HotlAuditSink
        // now ship; POST /v1/hotl/decisions returns 201 instead of 503
        // in production. The decision store always wires (table 0026 is
        // unconditional); the audit sink only wires when the audit
        // signing key env var is set (otherwise the route degrades to
        // "decision persisted, no audit trail" and logs a warning at the
        // route handler — matching the audit-disabled posture elsewhere).
        hotl_decision_store: Some(crate::hotl_bridge::PgHotlDecisionStore::arc(pool.clone())),
        hotl_audit: pg_audit_sink
            .as_ref()
            .map(|sink| crate::hotl_bridge::PgHotlAuditSink::arc(sink.clone())),
        // v1.2.4: outcome telemetry — PgOutcomesBackend implements both
        // writer and reader; construct once and coerce to each trait object.
        outcome_writer: Some({
            let backend: Arc<dyn xiaoguai_api::outcomes::OutcomeWriter> =
                Arc::new(crate::outcomes_bridge::PgOutcomesBackend::new(pool.clone()));
            backend
        }),
        outcomes_reader: Some({
            let backend: Arc<dyn xiaoguai_api::outcomes::OutcomesReader> =
                Arc::new(crate::outcomes_bridge::PgOutcomesBackend::new(pool.clone()));
            backend
        }),
        // v1.2.28: skill pack install/uninstall — PgSkillPackRepository.
        skill_packs: Some(crate::skills_bridge::PgSkillPackRepository::arc(
            pool.clone(),
        )),
        // v1.3.x: long-term memory — PgMemoryStore with the embedder selected by
        // `OLLAMA_HOST` (air-gapped Ollama vs in-process). Makes /v1/memories live.
        memory_store: Some(crate::memory_bridge::build_memory_store(pool.clone())),
        // v1.3.x: workspace CRUD — production wires PgWorkspaceRepository
        // in workspace_bridge.rs; `None` makes /v1/workspaces return 503.
        workspace_repository: None,
        // Sprint-8 S8-7 (DEC-023.3): skill-author production wiring.
        // Requires both the audit signing key (for the SkillAuditSink
        // adapter over PgAuditSink) and a running HotL enforcer. When
        // the audit key is unset we keep the four slots `None` — the
        // /v1/skills/proposals/* routes return 503 and `propose_skill`
        // stays unregistered.
        skill_proposals: pg_audit_sink
            .as_ref()
            .map(|_| xiaoguai_tasks::skill_author_pg::PgSkillProposalRepository::arc(pool.clone())),
        tenant_settings: pg_audit_sink
            .as_ref()
            .map(|_| xiaoguai_tasks::skill_author_pg::PgTenantSettings::arc(pool.clone())),
        skill_author_gate: pg_audit_sink.as_ref().map(|_| {
            crate::skill_author_bridge::EnforcerGateAdapter::arc(hotl_enforcer_arc.clone())
        }),
        skill_audit: pg_audit_sink
            .as_ref()
            .map(|sink| crate::skill_author_bridge::AuditSinkAdapter::arc(sink.clone())),
        skills_dir: std::env::var_os("XIAOGUAI_SKILLS_DIR").map_or_else(
            || {
                let home = std::env::var_os("HOME")
                    .map_or_else(|| std::path::PathBuf::from("."), std::path::PathBuf::from);
                home.join(".xiaoguai").join("skills")
            },
            std::path::PathBuf::from,
        ),
        // v1.8.0 (sprint-10b S10b-1): persona CRUD wired via the PG-backed
        // repository when a pool is available. `None` here would surface as
        // 503 from `/v1/personas/*`; production always has a Postgres pool.
        personas: Some(Arc::new(xiaoguai_personas::PgPersonaRepository::new(
            pool.clone(),
        ))),
        // v1.8.0 (sprint-10b S10b-5): watcher introspection — wire the
        // static (zero-watcher) adapter so the chat-ui WatchIndicator gets
        // a 200 + empty array instead of falling to its 404 fallback. A
        // session-aware WatchRunner adapter lands in a future sprint.
        watchers: Some(xiaoguai_api::StaticWatcherIntrospector::arc()),
        // Sprint-12 S12-4: shared with the gate adapter constructed
        // above. The registry is built once around line 378; both halves
        // see the same DashMap so resolves from the route handler reach
        // the gate's waiters.
        decision_registry: decision_registry.clone(),
    };

    // v0.7.4: mount the Feishu webhook with a PG-backed history store by
    // default (multi-replica safe). Operators can fall back to the
    // single-replica in-process store by setting
    // `XIAOGUAI_IM__USE_IN_PROCESS_HISTORY=true`.
    //
    // v1.1.3: DingTalk + WeCom mounts use the same history store as
    // Feishu — the store is keyed by `(provider, tenant, user, conv)`
    // so collisions across providers are impossible. Each `build_*_gateway`
    // helper returns `None` when the corresponding env vars are unset,
    // letting operators opt into one, two, or all three IM channels.
    let im_history = build_im_history(settings, &pool, &state, &default_model);
    let im_router = merge_routers(vec![
        build_feishu_gateway(&state, im_history.clone()),
        build_dingtalk_gateway(&state, im_history.clone()),
        build_wecom_gateway(&state, im_history.clone()),
    ]);

    let addr: SocketAddr = format!("{}:{}", settings.server.host, settings.server.port)
        .parse()
        .with_context(|| {
            format!(
                "parse bind addr {}:{}",
                settings.server.host, settings.server.port
            )
        })?;
    let (local, fut) =
        serve_with_state_and_extras(addr, state, im_router, settings.server.static_dir.clone())
            .await
            .context("bind api")?;
    tracing::info!(%local, "serve: api listening");

    // v1.1.6.2: notify systemd that all subsystems are up (Type=notify).
    // This also spawns the watchdog ping task when WATCHDOG_USEC is set;
    // the handle is aborted on shutdown so the tokio runtime can drain.
    crate::sd_notify_bridge::notify_ready();
    let watchdog_handle = crate::sd_notify_bridge::spawn_watchdog_ticker();

    tokio::select! {
        res = fut => res.context("axum serve")?,
        _ = tokio::signal::ctrl_c() => tracing::info!("serve: shutdown via ctrl-c"),
    }

    // v1.1.6.2: tell systemd we are shutting down before any cleanup.
    crate::sd_notify_bridge::notify_stopping();
    if let Some(h) = watchdog_handle {
        h.abort();
        let _ = h.await;
    }

    // v0.12.0: aborting the scheduler task cancels its tokio::select!
    // loop; in-flight fires complete naturally because run_to_completion
    // is awaited inside the task body.
    if let Some(h) = scheduler_handle {
        h.abort();
        let _ = h.await;
    }
    Ok(())
}

/// v0.7.4 / v1.1.3: build the shared `ImHistoryStore` used by every IM
/// mount. PG-backed by default for multi-replica safety; the in-process
/// `ConversationHistory` is an explicit opt-in via
/// `XIAOGUAI_IM__USE_IN_PROCESS_HISTORY=true`.
fn build_im_history(
    settings: &xiaoguai_config::Settings,
    pool: &sqlx::PgPool,
    state: &xiaoguai_api::AppState,
    default_model: &str,
) -> std::sync::Arc<dyn xiaoguai_im_gateway::ImHistoryStore> {
    use std::sync::Arc;
    use xiaoguai_im_gateway::{ConversationHistory, ImHistoryStore, PgImHistoryStore};
    use xiaoguai_storage::repositories::PgImIdentityRepository;

    if settings.im.use_in_process_history {
        tracing::info!(
            "serve: IM history using in-process ConversationHistory (XIAOGUAI_IM__USE_IN_PROCESS_HISTORY=true)"
        );
        let store: Arc<dyn ImHistoryStore> = Arc::new(ConversationHistory::new(
            settings.im.max_messages_per_conversation,
        ));
        store
    } else {
        tracing::info!(
            cap = settings.im.max_messages_per_conversation,
            "serve: IM history using PgImHistoryStore"
        );
        let store: Arc<dyn ImHistoryStore> = Arc::new(PgImHistoryStore::new(
            Arc::new(PgImIdentityRepository::new(pool.clone())),
            state.sessions.clone(),
            state.messages.clone(),
            default_model.to_string(),
            settings.im.max_messages_per_conversation,
        ));
        store
    }
}

/// Merge zero-or-more optional IM gateway routers into one. Returns
/// `None` when every input is `None` so the API router stays
/// unchanged when no IM channel is configured.
fn merge_routers(routers: Vec<Option<axum::Router>>) -> Option<axum::Router> {
    let mut combined: Option<axum::Router> = None;
    for r in routers.into_iter().flatten() {
        combined = Some(match combined {
            Some(acc) => acc.merge(r),
            None => r,
        });
    }
    combined
}

/// v0.7.4: assemble the Feishu IM gateway router. Returns `None` when
/// the operator hasn't supplied a Feishu signing key
/// (`XIAOGUAI_IM_FEISHU__VERIFICATION_TOKEN`); mounting the route with
/// an empty signing key would accept every payload.
fn build_feishu_gateway(
    state: &xiaoguai_api::AppState,
    history: std::sync::Arc<dyn xiaoguai_im_gateway::ImHistoryStore>,
) -> Option<axum::Router> {
    use std::sync::Arc;
    use xiaoguai_im_feishu::FeishuProvider;
    use xiaoguai_im_gateway::{mount_feishu_with_history, ImProvider};

    let signing_key = std::env::var("XIAOGUAI_IM_FEISHU__VERIFICATION_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())?;
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

/// v1.1.3: assemble the DingTalk IM gateway router. Returns `None` when
/// the operator hasn't supplied a DingTalk webhook signing secret
/// (`XIAOGUAI_IM_DINGTALK__APP_SECRET`); mounting with an empty secret
/// would accept every payload.
///
/// Reply path requires the trio `XIAOGUAI_IM_DINGTALK__APP_KEY`,
/// `XIAOGUAI_IM_DINGTALK__API_SECRET`, and `XIAOGUAI_IM_DINGTALK__ROBOT_CODE`.
/// When any reply-side env var is unset we mount the inbound webhook
/// anyway and stub the outbound reply — useful for soak-testing
/// signature + parser logic without needing `OpenAPI` credentials.
fn build_dingtalk_gateway(
    state: &xiaoguai_api::AppState,
    history: std::sync::Arc<dyn xiaoguai_im_gateway::ImHistoryStore>,
) -> Option<axum::Router> {
    use std::sync::Arc;
    use xiaoguai_im_dingtalk::DingTalkProvider;
    use xiaoguai_im_gateway::{mount_dingtalk_with_history, ImProvider};

    let webhook_secret = std::env::var("XIAOGUAI_IM_DINGTALK__APP_SECRET")
        .ok()
        .filter(|s| !s.is_empty())?;

    let app_key = std::env::var("XIAOGUAI_IM_DINGTALK__APP_KEY")
        .ok()
        .filter(|s| !s.is_empty());
    let api_secret = std::env::var("XIAOGUAI_IM_DINGTALK__API_SECRET")
        .ok()
        .filter(|s| !s.is_empty())
        // Operators frequently use the same value for both — fall back.
        .or_else(|| Some(webhook_secret.clone()));
    let robot_code = std::env::var("XIAOGUAI_IM_DINGTALK__ROBOT_CODE")
        .ok()
        .filter(|s| !s.is_empty());

    let provider: Arc<dyn ImProvider> = if let (Some(ak), Some(sec), Some(rc)) =
        (app_key, api_secret, robot_code)
    {
        match xiaoguai_im_dingtalk::HttpDingTalkClient::new() {
            Ok(client) => Arc::new(DingTalkProvider::with_api_sink(
                webhook_secret,
                Arc::new(client),
                ak,
                sec,
                rc,
            )),
            Err(e) => {
                tracing::error!(error = %e, "serve: HttpDingTalkClient build failed — falling back to stub reply sink");
                Arc::new(DingTalkProvider::new(webhook_secret))
            }
        }
    } else {
        tracing::warn!(
            "serve: XIAOGUAI_IM_DINGTALK__APP_KEY / __API_SECRET / __ROBOT_CODE incomplete — DingTalk replies will be stubbed"
        );
        Arc::new(DingTalkProvider::new(webhook_secret))
    };
    Some(mount_dingtalk_with_history(
        state.clone(),
        provider,
        history,
    ))
}

/// v1.1.3: assemble the WeCom IM gateway router. Returns `None` when
/// the operator hasn't supplied a WeCom callback token
/// (`XIAOGUAI_IM_WECOM__TOKEN`); mounting with an empty token would
/// accept every payload.
///
/// Reply path requires `XIAOGUAI_IM_WECOM__CORP_ID` + `__SECRET` +
/// `__AGENT_ID`. When any is unset we mount the inbound webhook and
/// stub outbound — same pattern as the DingTalk helper. The
/// `__AES_KEY` env var is reserved for the encrypted-payload variant
/// which is deferred (see v1.1.3 plan doc).
fn build_wecom_gateway(
    state: &xiaoguai_api::AppState,
    history: std::sync::Arc<dyn xiaoguai_im_gateway::ImHistoryStore>,
) -> Option<axum::Router> {
    use std::sync::Arc;
    use xiaoguai_im_gateway::{mount_wecom_with_history, ImProvider};
    use xiaoguai_im_wecom::WeComProvider;

    let token = std::env::var("XIAOGUAI_IM_WECOM__TOKEN")
        .ok()
        .filter(|s| !s.is_empty())?;

    let corp_id = std::env::var("XIAOGUAI_IM_WECOM__CORP_ID")
        .ok()
        .filter(|s| !s.is_empty());
    let secret = std::env::var("XIAOGUAI_IM_WECOM__SECRET")
        .ok()
        .filter(|s| !s.is_empty());
    let agent_id = std::env::var("XIAOGUAI_IM_WECOM__AGENT_ID")
        .ok()
        .and_then(|s| s.parse::<i64>().ok());

    if std::env::var("XIAOGUAI_IM_WECOM__AES_KEY")
        .ok()
        .filter(|s| !s.is_empty())
        .is_some()
    {
        tracing::warn!(
            "serve: XIAOGUAI_IM_WECOM__AES_KEY is set but encrypted payloads are not supported in v1.1.3 — disable EncodingAESKey in the WeCom admin console for now"
        );
    }

    let provider: Arc<dyn ImProvider> = if let (Some(cid), Some(sec), Some(aid)) =
        (corp_id, secret, agent_id)
    {
        match xiaoguai_im_wecom::HttpWeComClient::new() {
            Ok(client) => Arc::new(WeComProvider::with_api_sink(
                token,
                Arc::new(client),
                cid,
                sec,
                aid,
            )),
            Err(e) => {
                tracing::error!(error = %e, "serve: HttpWeComClient build failed — falling back to stub reply sink");
                Arc::new(WeComProvider::new(token))
            }
        }
    } else {
        tracing::warn!(
            "serve: XIAOGUAI_IM_WECOM__CORP_ID / __SECRET / __AGENT_ID incomplete — WeCom replies will be stubbed"
        );
        Arc::new(WeComProvider::new(token))
    };
    Some(mount_wecom_with_history(state.clone(), provider, history))
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
    static_dir: Option<String>,
) -> Result<(
    std::net::SocketAddr,
    impl std::future::Future<Output = std::io::Result<()>>,
)> {
    use tokio::net::TcpListener;

    let mut app = xiaoguai_api::router(state);
    if let Some(r) = extra {
        app = app.merge(r);
    }

    // Optional web-UI serving (chat-ui at `/`, admin-ui at `/admin/`). Only
    // when `server.static_dir` is set AND exists; otherwise the server stays
    // API-only. Mounted after the API router so `/v1` + `/healthz` win.
    if let Some(dir) = static_dir {
        let path = std::path::Path::new(&dir);
        if path.is_dir() {
            app = xiaoguai_api::static_ui::mount_static_ui(app, path);
            tracing::info!(static_dir = %dir, "serve: web UI mounted (chat-ui at /, admin-ui at /admin)");
        } else {
            tracing::warn!(
                static_dir = %dir,
                "serve: server.static_dir is set but not a directory; serving API only"
            );
        }
    }

    // v1.2.11: Prometheus + OTLP telemetry.
    // Gated on the `observability` Cargo feature so default builds are
    // unchanged. When enabled:
    //   - `GET /metrics` returns Prometheus text-format exposition.
    //   - All tracing spans are forwarded to the OTLP endpoint
    //     configured via `OTEL_EXPORTER_OTLP_ENDPOINT`
    //     (default `http://localhost:4317`).
    #[cfg(feature = "observability")]
    {
        app = xiaoguai_observability::mount(app).context("init observability")?;
        tracing::info!("serve: observability mounted (Prometheus + OTLP)");
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
///
/// ## Hybrid DB-backed merge (sprint-13 S13-10)
///
/// After the compiled-in CSV is loaded, we additionally pull rows from
/// the `casbin_rule` table (seeded by migration 0027) and merge them
/// into the in-memory enforcer. The hot-path check stays in memory;
/// the DB query happens exactly once per process. We then assert the
/// seeded `hotl:decide` rule is present — a partial migration that
/// failed to seed the row trips a panic at boot rather than letting an
/// un-enforceable route slip through. Tenants that explicitly disable
/// auth (dev mode) skip both the merge and the assertion.
async fn build_authz(
    settings: &Settings,
    pool: &sqlx::PgPool,
) -> Result<Option<std::sync::Arc<xiaoguai_auth::Authz>>> {
    use std::sync::Arc;
    if !settings.auth.required {
        return Ok(None);
    }
    let mut authz = xiaoguai_auth::Authz::new_default()
        .await
        .context("load casbin policy")?;

    // Pull rows seeded by migration 0027. Schema mirrors the Casbin
    // sql-adapter convention (`ptype`, `v0..v5`).
    let rows = sqlx::query_as::<
        _,
        (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ),
    >("SELECT ptype, v0, v1, v2, v3, v4, v5 FROM casbin_rule")
    .fetch_all(pool)
    .await
    .context("load casbin_rule rows")?;

    let merged: Vec<xiaoguai_auth::DbPolicyRow> = rows
        .into_iter()
        .map(
            |(ptype, v0, v1, v2, v3, v4, v5)| xiaoguai_auth::DbPolicyRow {
                ptype,
                v0,
                v1,
                v2,
                v3,
                v4,
                v5,
            },
        )
        .collect();
    let merged_count = merged.len();
    authz
        .merge_db_policies(merged)
        .await
        .context("merge casbin_rule rows")?;
    tracing::info!(merged_count, "serve: Casbin DB-merged rows loaded");

    // Defensive boot-time assertion (DEC-HLD-016 / S13-10): the seeded
    // `hotl:decide` scope rule MUST be present after the merge. If it's
    // missing, migration 0027 ran partially (or not at all) and the
    // hotl decisions route would silently allow anyone — fail fast.
    let required = ["hotl:decide", "/v1/hotl/decisions", "POST", "allow"];
    if !authz.has_policy_rule(&required).await {
        anyhow::bail!(
            "Casbin policy missing required rule: (p, {required:?}). \
             Did migration 0027_hotl_escalations_split.sql run? \
             See DEC-HLD-016 / sprint-13 S13-10."
        );
    }
    tracing::info!("serve: Casbin hotl:decide rule asserted present");

    Ok(Some(Arc::new(authz)))
}

/// Whether to scrub PII/secrets from audit entries before signing.
///
/// On by default (the enterprise privacy posture); set
/// `XIAOGUAI_AUDIT_REDACT_PII` to `false`/`0`/`no`/`off` to disable.
fn audit_redaction_enabled() -> bool {
    match std::env::var("XIAOGUAI_AUDIT_REDACT_PII") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// v0.11.2 — assemble the `EvalService` so the admin pane can run
/// suites and convert prod runs into regression cases. We always wire
/// it (the suites directory may be empty; the list endpoint returns an
/// empty array, the run endpoint returns 400 for missing suites). The
/// case-from-session source reads `sessions` + `audit_log` directly.
fn build_eval_service(
    settings: &Settings,
    pool: sqlx::PgPool,
) -> std::sync::Arc<xiaoguai_api::EvalService> {
    use std::path::PathBuf;
    use std::sync::Arc;
    use xiaoguai_api::EvalService;
    use xiaoguai_eval::{DefaultEvalAgentBuilder, EvalRunner};

    let suites_dir = PathBuf::from(settings.eval.suites_dir.clone());
    let runner = EvalRunner::new(Arc::new(DefaultEvalAgentBuilder::new(
        settings.eval.max_iterations,
    )));
    let source = Arc::new(crate::eval_bridge::PgCaseFromSessionSource::new(pool));
    tracing::info!(
        suites_dir = %suites_dir.display(),
        max_iterations = settings.eval.max_iterations,
        "serve: EvalService wired"
    );
    Arc::new(EvalService::new(runner, suites_dir, source))
}

/// Sprint-8 S8-5 (DEC-023.1): refuse-to-start when MCP OAuth tokens exist
/// in the DB but no encryption keyring is configured.
///
/// A fresh deployment with zero rows boots without an encryption key —
/// operators register their first MCP OAuth server and the encrypted
/// column populates from then on. Existing rows imply the operator has
/// been running with cleartext-or-encrypted tokens that we must be able
/// to read/refresh, so the keyring is now required.
///
/// # Errors
/// Returns an error if the `mcp_oauth_tokens` table has ≥ 1 row AND
/// `XIAOGUAI_MCP_OAUTH_TOKEN_KEY` cannot be loaded.
async fn check_mcp_oauth_keyring(pool: &sqlx::PgPool) -> Result<()> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mcp_oauth_tokens")
        .fetch_one(pool)
        .await
        .context("pg count mcp_oauth_tokens")?;
    if count == 0 {
        tracing::debug!(
            "mcp_oauth_tokens empty; skipping keyring requirement (fresh install path)"
        );
        return Ok(());
    }
    match xiaoguai_mcp::auth::Keyring::from_env() {
        Ok(_) => {
            tracing::info!(
                rows = count,
                "mcp_oauth_tokens keyring loaded; refresh-token encryption-at-rest active"
            );
            Ok(())
        }
        Err(e) => Err(anyhow::anyhow!(
            "mcp_oauth_tokens contains {count} row(s) but the encryption keyring is unavailable: {e}.\n\
             Set XIAOGUAI_MCP_OAUTH_TOKEN_KEY (32-byte base64url AES-256-GCM key) and restart."
        )),
    }
}
