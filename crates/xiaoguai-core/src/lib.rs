//! Xiaoguai core — library + binary wiring.
//!
//! Loads configuration, opens the `SQLite` store, applies migrations,
//! initializes JWT + RBAC + audit chain, then either runs the API server
//! (default) or executes a single subcommand (e.g. `smoke`).
//!
//! The boot flow lives in [`run_with_cli`] so both the legacy `xiaoguai-core`
//! binary and the unified `xiaoguai serve` subcommand can drive it.

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

pub mod acp_bridge;
mod audit_bridge;
pub mod banners;
pub mod coding_bridge;
mod eval_bridge;
pub mod hotl_bridge;
// T7.2: pub so the CLI (`xiaoguai memory import/export`) reuses
// `build_memory_store` — one embedder-selection source of truth.
pub mod memory_bridge;
pub mod outcomes_bridge;
#[cfg(feature = "packs")]
pub mod pack_runtime;
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
use xiaoguai_config::Settings;
use xiaoguai_storage::{db, ReadWritePool};

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
        cfg.auth_enabled = settings.auth.is_enabled(),
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
    tracing::info!("smoke: opening the SQLite store");
    let pool = db::connect(&settings.database.url, settings.database.max_connections)
        .await
        .context("db connect")?;
    db::migrate(&pool).await.context("db migrate")?;
    let row: (i32,) = sqlx::query_as("SELECT 1")
        .fetch_one(&pool)
        .await
        .context("db select 1")?;
    anyhow::ensure!(row.0 == 1, "db select 1 returned {}", row.0);
    tracing::info!("smoke: db ok");

    tracing::info!(
        auth_enabled = settings.auth.is_enabled(),
        "smoke: owner-auth gate configured (DEC-033: no OIDC/RBAC)"
    );

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
        AppState, CancelRegistry,
    };
    use xiaoguai_audit::chain::sink::SqliteAuditSink;
    use xiaoguai_llm::{build_router, LlmBackend, MockBackend, OsEnvResolver};
    use xiaoguai_mcp::McpSupervisor;
    #[cfg(feature = "observability")]
    use xiaoguai_observability;
    use xiaoguai_storage::repositories::{
        LlmProviderRepository, SqliteLlmProviderRepository, SqliteMcpServerRepository,
        SqliteMessageRepository, SqliteSessionRepository,
    };

    use crate::audit_bridge::SqliteAuditAdapter;

    tracing::info!("serve: opening the SQLite store");
    let pool = db::connect(&settings.database.url, settings.database.max_connections)
        .await
        .context("db connect")?;
    db::migrate(&pool).await.context("db migrate")?;

    // Sprint-8 S8-5: refuse-to-start when MCP OAuth tokens exist but the
    // encryption keyring is unavailable. Fresh-install path (empty table)
    // boots without the env var.
    check_mcp_oauth_keyring(&pool).await?;

    // SQLite is single-writer/single-file (DEC-033) — no replicas to route to.
    let rw_pool = ReadWritePool::new(pool.clone());

    // v0.6.2: read system-wide LLM providers and assemble a router. The
    // resulting `LlmRouter` implements `LlmBackend`, so it drops in
    // wherever the old `MockBackend` used to live. If the registry is
    // empty we keep the `MockBackend` fallback so that fresh deployments
    // still boot and serve a deterministic response.
    let provider_repo = SqliteLlmProviderRepository::from_env(pool.clone())
        .context("load at-rest encryption key")?;
    // Opt-in encryption-at-rest backfill (mirrors the SEC-19 webhook-token
    // backfill): when XIAOGUAI_AT_REST_KEY is configured, seal any provider
    // api_key still stored cleartext. Non-fatal — a failure just leaves those
    // rows cleartext until the next boot.
    if let Err(e) = provider_repo.backfill_encrypt_api_keys().await {
        tracing::warn!(
            error = %e,
            "llm provider api_key encryption-at-rest backfill failed (non-fatal)"
        );
    }
    let mut rows = provider_repo
        .list()
        .await
        .context("db list llm providers")?;
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
    // Test/e2e affordance: force the deterministic MockBackend regardless of
    // the seeded provider rows. The default-seeded providers (ollama-local,
    // minimax) need a running Ollama / an API key, so a hermetic e2e or smoke
    // stack can't produce a real reply — set XIAOGUAI_LLM__MOCK=true to get a
    // deterministic one. NEVER set in production.
    let force_mock = std::env::var("XIAOGUAI_LLM__MOCK")
        .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
        .unwrap_or(false);
    let (backend, default_model): (Arc<dyn LlmBackend>, String) = if rows.is_empty() || force_mock {
        if force_mock {
            tracing::warn!(
                "serve: XIAOGUAI_LLM__MOCK set — using deterministic MockBackend (test/e2e only)"
            );
        } else {
            tracing::warn!(
                "serve: llm_providers table is empty — falling back to MockBackend. \
                 Use `xiaoguai provider register` to populate it."
            );
            // T8.4: loud operator banner for the implicit fallback. The
            // fallback behaviour itself is unchanged (tests/e2e rely on it);
            // explicit XIAOGUAI_LLM__MOCK=true opt-in stays quiet (other arm).
            eprintln!("{}", banners::empty_providers_banner());
        }
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

    // SEC-01: refuse to start when binding a non-loopback interface with the
    // owner auth gate disabled. The old behaviour (warn + continue) shipped an
    // unauthenticated `/v1/**` on 0.0.0.0 by default. Operators who genuinely
    // front the service with a trusted reverse proxy (and accept the risk) can
    // opt back in with `XIAOGUAI_ALLOW_UNAUTHENTICATED_NONLOOPBACK=1`; the
    // container image sets it so the bundled quickstart keeps working.
    if auth.is_none()
        && !host_is_loopback(&settings.server.host)
        && !allow_unauthenticated_nonloopback()
    {
        anyhow::bail!(
            "refusing to start: binding non-loopback host `{host}` with owner auth DISABLED \
             would expose the entire /v1 API to the network unauthenticated. Fix one of:\n  \
             1) set auth.username + auth.password (XIAOGUAI_AUTH__USERNAME / __PASSWORD), or\n  \
             2) bind loopback (XIAOGUAI_SERVER__HOST=127.0.0.1), or\n  \
             3) (NOT recommended — only behind a trusted proxy) set \
             XIAOGUAI_ALLOW_UNAUTHENTICATED_NONLOOPBACK=1",
            host = settings.server.host,
        );
    }

    // v0.6.5: try to assemble the production audit bridge. The signing
    // key lives in the env var named by `settings.audit.signing_key_env`
    // — empty / missing means audit endpoints stay at 503 in production
    // rather than silently using `settings.audit.hmac_key` (which is the
    // dev-only fallback wired into `smoke`).
    // Sprint-8 S8-7: hoist the SqliteAuditSink so the skill_author_bridge can
    // reuse the same signing chain. `None` here keeps skill-author audit
    // wiring off when the signing key env var is empty.
    let pg_audit_sink: Option<Arc<SqliteAuditSink>> =
        match std::env::var(&settings.audit.signing_key_env) {
            Ok(key) if !key.is_empty() => {
                // Redaction MUST be wired here too: this is the PRIMARY sink
                // feeding the audit reader/verifier/exporter, HotL, coding, and
                // skill-author paths. Without it, PII/secrets land un-redacted in
                // `audit_log` and in every compliance export — even though
                // redaction defaults ON. (The scheduler sink below mirrors this;
                // redaction is idempotent so the two sinks stay consistent.)
                let mut sink = SqliteAuditSink::new(pool.clone(), key.into_bytes());
                if audit_redaction_enabled() {
                    sink = sink.with_redactor(xiaoguai_audit::Redactor::new());
                }
                Some(Arc::new(sink))
            }
            _ => None,
        };

    let (audit_reader, audit_verifier, audit_chain_exporter): (
        Option<Arc<dyn AuditReader>>,
        Option<Arc<dyn AuditVerifier>>,
        Option<Arc<dyn xiaoguai_api::audit::AuditChainExporter>>,
    ) = if let Some(sink) = &pg_audit_sink {
        let adapter = Arc::new(SqliteAuditAdapter::new(sink.clone()));
        tracing::info!(
            env = %settings.audit.signing_key_env,
            "serve: audit reader+verifier+exporter wired (SqliteAuditSink)"
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
        Arc::new(SqliteMcpServerRepository::new(pool.clone()));

    // v0.12.0: scheduler bootstrap. Off by default so existing
    // deployments don't change behaviour. When enabled we spawn a
    // `JobRunner::run_loop` on a tokio task, wire the PG repositories,
    // run the agent loop through `RuntimeJobExecutor`, and hand the
    // `WebhookSource` to `AppState` so `/v1/admin/scheduler/webhooks/...`
    // can fire reactive jobs.
    //
    // v0.12.1: also wire the `SqliteScheduledSessionWriter` into the
    // executor (so `scheduled_job_runs.session_id` populates and the
    // audit-first console can drill into transcripts) and the
    // `SqliteScheduledJobUpserter` into AppState for `POST /v1/admin/scheduler/jobs`.
    // Register the governed coding tools (DEC-034) into the agent toolbox so the
    // ReAct loop can edit/commit/rollback in-loop — HotL-gated on `tool_call.*`
    // by the loop, checkpointed + `code.*`-audited by the coding bridge.
    //
    // Opt-in by design (security review): coding tools register ONLY when the
    // operator points `XIAOGUAI_CODING_WORKSPACE` at a directory AND an audit
    // signing key is configured. Unset workspace ⇒ no tools and no `git init` of
    // the server's CWD (H1); unset signing key ⇒ no ungoverned coding. The
    // egress tools (`git_push`/`open_pr`) need a further `XIAOGUAI_CODING_ALLOW_
    // EGRESS` opt-in (C1).
    let toolbox = {
        match (
            &pg_audit_sink,
            crate::coding_bridge::coding_workspace_root(),
        ) {
            (Some(sink), Some(root)) => {
                let allow_egress = crate::coding_bridge::coding_allow_egress();
                match crate::coding_bridge::build_coding_toolbox(sink.clone(), &root, allow_egress)
                    .await
                {
                    Ok(tb) => {
                        tracing::info!(
                            workspace = %root.display(),
                            tools = tb.len(),
                            egress = allow_egress,
                            "serve: governed coding tools registered into the agent toolbox"
                        );
                        Arc::new(tb)
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "serve: failed to build coding toolbox — agent runs without coding tools"
                        );
                        Arc::new(Toolbox::new())
                    }
                }
            }
            (None, Some(_)) => {
                tracing::info!(
                    "serve: coding workspace set but audit signing key unset — coding tools \
                     NOT registered (no ungoverned coding)"
                );
                Arc::new(Toolbox::new())
            }
            _ => {
                tracing::info!(
                    "serve: coding tools disabled (set XIAOGUAI_CODING_WORKSPACE to a directory \
                     to enable governed in-loop coding)"
                );
                Arc::new(Toolbox::new())
            }
        }
    };

    // Tier-2 prereq: build the HOTL enforcer once, share between
    // `AppState.hotl_enforcer` (gating LLM calls upstream in
    // `send_message`) and `agent_defaults.hotl_gate` (gating each tool
    // dispatch inside the ReAct loop). The enforcer is fail-closed on PG
    // errors; `EnforcerGate` maps that into a per-tool `Deny` verdict.
    //
    // Sharing one PG-backed enforcer means the budget counter is unified:
    // a tenant that's burned its `tool_call.*` budget can still call the
    // LLM (different scope), and vice versa.
    let hotl_policy_store_pg =
        Arc::new(crate::hotl_bridge::SqliteHotlPolicyStore::new(pool.clone()));
    let hotl_enforcer_arc: Arc<dyn xiaoguai_api::hotl::enforcer::HotlEnforcer> = Arc::new(
        crate::hotl_bridge::SqliteHotlEnforcer::new(pool.clone(), hotl_policy_store_pg.clone()),
    );
    // Sprint-12 S12-4 / Sprint-13 S13-5: the `DecisionRegistry` is
    // constructed ONCE here and shared between the gate adapter (so
    // `SuspendingHotlGate::check` can mint tickets against it) and
    // `AppState.decision_registry` (so `POST /v1/hotl/decisions` can
    // resolve waiters on it). A second registry would silently no-op
    // resolves and hang the loop until the 24h default expiry.
    //
    // Sprint-13 S13-5: the registry is wired to
    // `SqliteHotlEscalationRepository` and uses `replay_from_storage` so any
    // `hotl_pending` rows that survived a restart are reattached BEFORE
    // the HTTP server starts accepting requests. The replay log line
    // `hotl: replayed N pending decision waiters from PG` is the SRE
    // signal that the boot recovery path actually ran.
    let hotl_escalation_store: std::sync::Arc<
        dyn xiaoguai_storage::repositories::HotlEscalationStore,
    > = std::sync::Arc::new(
        xiaoguai_storage::repositories::SqliteHotlEscalationRepository::new(pool.clone()),
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
    // Sprint-13 S13-6: wire the `SqliteHotlRedactionRepo` + per-tenant
    // policy required flag + audit sink into the suspend gate so
    // operator banners see masked tool args and the audit chain carries
    // the matched policy id.
    let hotl_redaction_repo: Arc<
        dyn xiaoguai_storage::repositories::hotl_redaction::HotlRedactionRepo,
    > = Arc::new(
        xiaoguai_storage::repositories::hotl_redaction::SqliteHotlRedactionRepo::new(pool.clone()),
    );
    let hotl_gate_audit_sink: Option<Arc<dyn xiaoguai_api::hotl::audit::HotlAuditSink>> =
        pg_audit_sink
            .as_ref()
            .map(|sink| crate::hotl_bridge::SqliteHotlAuditSink::arc(sink.clone()));
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
        Arc::new(SqliteSessionRepository::new(pool.clone()));
    let pg_message_repo: Arc<dyn xiaoguai_storage::repositories::MessageRepository> =
        Arc::new(SqliteMessageRepository::new(pool.clone()));
    // v0.12.x.1: also wire `SqliteScheduledJobsReader` (admin-ui Scheduler
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
            Arc::new(crate::scheduler_bridge::SqliteScheduledSessionWriter::new(
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
        let pg_jobs = Arc::new(xiaoguai_scheduler::SqliteJobRepository::new(pool.clone()));
        let jobs: Arc<dyn xiaoguai_scheduler::JobRepository> = pg_jobs.clone();
        let runs: Arc<dyn xiaoguai_scheduler::JobRunRepository> = Arc::new(
            xiaoguai_scheduler::SqliteJobRunRepository::new(pool.clone()),
        );
        // Audit appender: route through the same SqliteAuditSink the audit
        // bridge already constructed when the signing key was present.
        // When audit is unwired we fall back to NullAuditAppender so the
        // scheduler still runs (audit gap is logged at startup).
        let audit_appender: Arc<dyn xiaoguai_scheduler::AuditAppender> = match std::env::var(
            &settings.audit.signing_key_env,
        ) {
            Ok(key) if !key.is_empty() => {
                let mut sink = SqliteAuditSink::new(pool.clone(), key.into_bytes());
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
                Arc::new(crate::scheduler_bridge::SqliteSchedulerAuditAppender::new(
                    sink,
                ))
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
        let upserter: Arc<dyn xiaoguai_api::scheduler::ScheduledJobUpserter> =
            Arc::new(crate::scheduler_bridge::SqliteScheduledJobUpserter::new(
                pg_jobs.clone(),
                Some(webhook_source.clone()),
            ));
        // v0.12.x.1: admin-ui Scheduler pane reader + "Run now" handle.
        let jobs_reader: Arc<dyn xiaoguai_api::scheduler::ScheduledJobsReader> = Arc::new(
            crate::scheduler_bridge::SqliteScheduledJobsReader::new(pg_jobs, runner.clone()),
        );
        // v0.12.x.1: per-tenant webhook tokens — PG validator + admin.
        let token_validator: Arc<dyn xiaoguai_api::scheduler::WebhookTokenValidator> = Arc::new(
            crate::scheduler_bridge::SqliteWebhookTokenValidator::new(pool.clone()),
        );
        let token_admin: Arc<dyn xiaoguai_api::scheduler::WebhookTokenAdmin> = Arc::new(
            crate::scheduler_bridge::SqliteWebhookTokenAdmin::new(pool.clone()),
        );
        // SEC-19: one-time, idempotent — hash any pre-existing plaintext webhook
        // tokens in place so the store holds only digests. Non-fatal: a failure
        // just leaves legacy rows plaintext (they still validate) until retried.
        if let Err(e) = crate::scheduler_bridge::backfill_webhook_token_hashes(&pool).await {
            tracing::warn!(error = %e, "SEC-19 webhook token hash backfill failed (non-fatal)");
        }
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

    // T6 self-healing (GLUE-1): incident persistence over the shared pool.
    // Held in a local (not just the AppState field) so the boot-time
    // reconcile below (#284) can run against it before serving.
    let incident_store = Arc::new(xiaoguai_api::incident_store::SqliteIncidentStore::new(
        pool.clone(),
    ));

    let state = AppState {
        sessions: pg_session_repo.clone(),
        messages: pg_message_repo.clone(),
        backend,
        toolbox: toolbox.clone(),
        agent_defaults: agent_defaults.clone(),
        cancels: Arc::new(CancelRegistry::new()),
        mcp_servers: Some(mcp_servers_repo),
        auth,
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
        today: Some(crate::today_bridge::SqliteTodayReader::arc(rw_pool.clone())),
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
        session_forker: Some(crate::sessions_bridge::SqliteSessionForker::arc(
            pg_session_repo.clone(),
        )),
        // v1.1.1: token-usage aggregator backing /v1/usage and the
        // admin-ui Usage pane (plus the Today pane's 24h summary card).
        // Always wired in production — the underlying token_usage table
        // is unconditional (migration 0004).
        usage_reader: Some(crate::usage_bridge::SqliteUsageReader::arc(rw_pool.clone())),
        // v0.12.x.1: per-tenant webhook token validator + admin CRUD
        // + admin-ui Scheduler pane jobs reader. All `None` when the
        // scheduler is disabled — the matching routes return 503.
        webhook_token_validator,
        webhook_token_admin,
        scheduler_jobs_reader,
        // v1.2.3: HOTL boundary policy — SqliteHotlPolicyStore + SqliteHotlEnforcer
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
        hotl_decision_store: Some(crate::hotl_bridge::SqliteHotlDecisionStore::arc(
            pool.clone(),
        )),
        hotl_audit: pg_audit_sink
            .as_ref()
            .map(|sink| crate::hotl_bridge::SqliteHotlAuditSink::arc(sink.clone())),
        // v1.2.4: outcome telemetry — SqliteOutcomesBackend implements both
        // writer and reader; construct once and coerce to each trait object.
        outcome_writer: Some({
            let backend: Arc<dyn xiaoguai_api::outcomes::OutcomeWriter> = Arc::new(
                crate::outcomes_bridge::SqliteOutcomesBackend::new(pool.clone()),
            );
            backend
        }),
        outcomes_reader: Some({
            let backend: Arc<dyn xiaoguai_api::outcomes::OutcomesReader> = Arc::new(
                crate::outcomes_bridge::SqliteOutcomesBackend::new(pool.clone()),
            );
            backend
        }),
        // v1.2.28: skill pack install/uninstall — SqliteSkillPackRepository.
        skill_packs: Some(crate::skills_bridge::SqliteSkillPackRepository::arc(
            pool.clone(),
        )),
        // v1.3.x: long-term memory — SqliteMemoryStore with the embedder selected by
        // the `memory.embedder` config block (DEC-036), `OLLAMA_HOST` env overriding.
        memory_store: Some(crate::memory_bridge::build_memory_store(
            pool.clone(),
            &settings.memory.embedder,
        )),
        // v1.3.x: workspace CRUD — production wires SqliteWorkspaceRepository
        // in workspace_bridge.rs; `None` makes /v1/workspaces return 503.
        workspace_repository: None,
        // Sprint-8 S8-7 (DEC-023.3): skill-author production wiring.
        // Requires both the audit signing key (for the SkillAuditSink
        // adapter over SqliteAuditSink) and a running HotL enforcer. When
        // the audit key is unset we keep the four slots `None` — the
        // /v1/skills/proposals/* routes return 503 and `propose_skill`
        // stays unregistered.
        skill_proposals: pg_audit_sink.as_ref().map(|_| {
            xiaoguai_tasks::skill_author_sqlite::SqliteSkillProposalRepository::arc(pool.clone())
        }),
        tenant_settings: pg_audit_sink
            .as_ref()
            .map(|_| xiaoguai_tasks::skill_author_sqlite::SqliteTenantSettings::arc(pool.clone())),
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
        // 503 from `/v1/personas/*`; production always has a SQLite pool.
        personas: Some(Arc::new(xiaoguai_personas::SqlitePersonaRepository::new(
            pool.clone(),
        ))),
        // v1.8.0 (sprint-10b S10b-5): watcher introspection — wire the
        // static (zero-watcher) adapter so the chat-ui WatchIndicator gets
        // a 200 + empty array instead of falling to its 404 fallback. A
        // session-aware WatchRunner adapter lands in a future sprint.
        watchers: Some(xiaoguai_api::StaticWatcherIntrospector::arc()),
        // /loop wiring lands just below — the controller needs an AppState
        // clone with `loops = None` so a tick's `run_turn` never re-enters
        // the controller. Set to `None` here, then overwritten after the
        // controller is built from this state's clone.
        loops: None,
        // T3 expert center: team CRUD + session attachment, sharing the
        // personas pool. Audit reuses the same chain adapter as hotl_audit
        // (entries differ only by action namespace `team.*`).
        teams: Some(Arc::new(xiaoguai_personas::SqliteTeamRepository::new(
            pool.clone(),
        ))),
        team_audit: pg_audit_sink
            .as_ref()
            .map(|sink| crate::hotl_bridge::SqliteHotlAuditSink::arc(sink.clone())),
        // T6 self-healing (GLUE-1): incident persistence over the shared
        // pool (constructed above so the #284 boot reconcile can reuse
        // it). Ingest audit reuses the team_audit chain adapter above
        // (the sink is feature-generic; entries are `incident.*`).
        incidents: Some(incident_store.clone()),
        // Sprint-12 S12-4: shared with the gate adapter constructed
        // above. The registry is built once around line 378; both halves
        // see the same DashMap so resolves from the route handler reach
        // the gate's waiters.
        decision_registry: decision_registry.clone(),
    };

    // /loop L1 (DEC-039): wire the LoopController over the `loops` table.
    // It captures the AppState exactly as built above (`loops = None`), so
    // a tick's `run_turn` runs through the same pipeline as a chat turn
    // without ever calling back into the controller. Boot-replay re-arms
    // every unexpired `active` loop BEFORE the HTTP server accepts
    // requests — same "survives restart" semantics as the HotL decision
    // registry replay above. The log line is the SRE signal it ran.
    let loop_store: Arc<dyn xiaoguai_storage::repositories::LoopStore> = Arc::new(
        xiaoguai_storage::repositories::SqliteLoopRepository::new(pool.clone()),
    );
    // L3 Part C: wire the token-usage ledger so the loop's max_total_tokens
    // budget can sum the session's spend since loop-start.
    let loop_token_usage: Arc<dyn xiaoguai_storage::repositories::TokenUsageRepository> =
        Arc::new(xiaoguai_storage::repositories::SqliteTokenUsageRepository::new(pool.clone()));
    let loop_controller =
        xiaoguai_api::LoopController::new(loop_store, state.clone(), Some(loop_token_usage));
    match loop_controller.replay_from_storage().await {
        Ok((armed, expired)) => {
            tracing::info!(armed, expired, "loop: replayed loops from storage");
        }
        Err(e) => {
            tracing::error!(
                ?e,
                "loop: boot replay failed — serving without re-armed loops"
            );
        }
    }
    let mut state = state;
    state.loops = Some(loop_controller);

    // #284: incident boot reconcile — the analyze/approve agent turns run
    // inside the HTTP request, so a crash or dropped handler future
    // strands incidents on `analyzing`/`repairing` forever (no
    // `analyzing → analyzing` retry exists). Mirror the loop boot-replay
    // posture above: reconcile BEFORE the server accepts requests —
    // `analyzing → open` (retryable), `repairing → failed` (the Executor
    // may have applied partial mutations; surface, don't retry) — and
    // audit each move through the same chain as the other `incident.*`
    // entries. Best-effort: a reconcile failure must not block serving.
    match xiaoguai_api::incident_store::IncidentStore::reconcile_interrupted(&*incident_store).await
    {
        Ok(reconciled) => {
            if !reconciled.is_empty() {
                tracing::warn!(
                    count = reconciled.len(),
                    "incidents: reconciled interrupted incidents at boot"
                );
            }
            if let Some(sink) = state.team_audit.clone() {
                for r in &reconciled {
                    let entry = xiaoguai_audit::AuditEntry {
                        ts: chrono::Utc::now(),
                        tenant_id: xiaoguai_audit::OWNER_TENANT_ID.to_string(),
                        actor: "system".to_string(),
                        action: "incident.reconciled".to_string(),
                        resource: Some(format!("incident:{}", r.id)),
                        details: serde_json::json!({
                            "from": r.from,
                            "to": r.to,
                            "reason": "interrupted, reconciled at boot",
                        }),
                    };
                    if let Err(e) = sink.append(entry).await {
                        tracing::warn!(
                            error = %e, incident_id = %r.id,
                            "incidents: boot-reconcile audit append failed (non-blocking)"
                        );
                    }
                }
            }
        }
        Err(e) => {
            tracing::error!(
                error = %e,
                "incidents: boot reconcile failed — stranded analyzing/repairing rows may remain"
            );
        }
    }

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
    // Web-UI LLM provider management (list/create/delete). A self-contained
    // router merged outside the `v1` auth layer, so it MUST re-apply the owner
    // gate itself — these endpoints rewrite the LLM endpoint a stored API key
    // is sent to (an SSRF / key-exfil surface), so leaving them open even when
    // the owner configured auth is a real hole (security review). Mirror the
    // `/v1/mcp/serve` pattern: gate when auth is set, warn loudly when not.
    let providers_router = {
        let r = xiaoguai_api::routes::providers::build_router(Arc::new(
            SqliteLlmProviderRepository::from_env(pool.clone())
                .context("load at-rest encryption key")?,
        ));
        if let Some(validator) = state.auth.clone() {
            r.route_layer(axum::middleware::from_fn(move |req, next| {
                let v = validator.clone();
                async move { xiaoguai_api::auth::require_auth(v, req, next).await }
            }))
        } else {
            tracing::warn!(
                "provider management (/v1/admin/providers) is UNAUTHENTICATED — no owner \
                 auth configured. Set auth.username/password (XIAOGUAI_AUTH__*) before \
                 exposing this service; these endpoints can repoint where API keys are sent."
            );
            r
        }
    };
    let im_router = merge_routers(vec![
        build_feishu_gateway(&state, im_history.clone()),
        build_dingtalk_gateway(&state, im_history.clone()),
        build_wecom_gateway(&state, im_history.clone()),
        Some(providers_router),
    ]);

    let addr: SocketAddr = format!("{}:{}", settings.server.host, settings.server.port)
        .parse()
        .with_context(|| {
            format!(
                "parse bind addr {}:{}",
                settings.server.host, settings.server.port
            )
        })?;
    let (local, fut) = match serve_with_state_and_extras(
        addr,
        state,
        im_router,
        settings.server.static_dir.clone(),
    )
    .await
    {
        Ok(bound) => bound,
        // T8.1: an occupied port is the most common fresh-install failure —
        // print the three remedies instead of a bare anyhow chain.
        Err(e) if banners::is_addr_in_use(&e) => {
            eprintln!(
                "{}",
                banners::addr_in_use_message(&settings.server.host, settings.server.port)
            );
            return Err(e.context("bind api: address already in use"));
        }
        Err(e) => return Err(e.context("bind api")),
    };
    tracing::info!(%local, "serve: api listening");
    // T8.1: post-bind operator banner on stdout (the tracing line above is
    // telemetry; this is the human-facing "it worked, do this next"). Resolve
    // the static dir the same way the mount path does so the banner only
    // promises a web UI when one is actually served — pip / source installs are
    // API-only and `{url}/` 404s (recomputing is a few cheap path stats).
    let has_web_ui = resolve_static_dir(settings.server.static_dir.as_deref()).is_some();
    println!("{}", banners::serve_banner(&local, has_web_ui));

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
    pool: &sqlx::SqlitePool,
    state: &xiaoguai_api::AppState,
    default_model: &str,
) -> std::sync::Arc<dyn xiaoguai_im_gateway::ImHistoryStore> {
    use std::sync::Arc;
    use xiaoguai_im_gateway::{ConversationHistory, ImHistoryStore, SqliteImHistoryStore};
    use xiaoguai_storage::repositories::SqliteImIdentityRepository;

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
            "serve: IM history using SqliteImHistoryStore"
        );
        let store: Arc<dyn ImHistoryStore> = Arc::new(SqliteImHistoryStore::new(
            Arc::new(SqliteImIdentityRepository::new(pool.clone())),
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

/// Resolve which directory (if any) holds the web UI to serve.
///
/// Precedence:
///   1. An explicit `server.static_dir`. A non-empty value that is a real
///      directory is used; an empty string is an explicit opt-out (API-only);
///      a non-empty value that isn't a directory warns and falls through to
///      API-only.
///   2. When unset, probe the conventional bundle locations so native installs
///      (.deb/.rpm/tarball ship the built UI under `share/xiaoguai/static`,
///      next to the binary) serve it with zero configuration. The first
///      candidate that exists and contains a `chat-ui` sub-directory wins.
fn resolve_static_dir(configured: Option<&str>) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};

    let has_ui = |dir: &Path| dir.is_dir() && dir.join("chat-ui").is_dir();

    if let Some(raw) = configured {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None; // explicit opt-out
        }
        let path = PathBuf::from(trimmed);
        if path.is_dir() {
            return Some(path);
        }
        tracing::warn!(
            static_dir = %trimmed,
            "serve: server.static_dir is set but not a directory; serving API only"
        );
        return None;
    }

    // Unset → probe conventional locations relative to the binary, then system
    // share dirs. Candidates are ordered most- to least-specific.
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(bin_dir) = exe.parent() {
            candidates.push(bin_dir.join("static")); // exe-adjacent (Docker WORKDIR-style)
                                                     // tarball/.deb/.rpm: <prefix>/bin/xiaoguai-core + <prefix>/share/xiaoguai/static
            candidates.push(bin_dir.join("../share/xiaoguai/static"));
        }
    }
    candidates.push(PathBuf::from("/usr/local/share/xiaoguai/static"));
    candidates.push(PathBuf::from("/usr/share/xiaoguai/static"));

    candidates.into_iter().find(|c| has_ui(c))
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

    // Optional web-UI serving (chat-ui at `/`, admin-ui at `/admin/`), mounted
    // after the API router so `/v1` + `/healthz` win. An explicit
    // `server.static_dir` wins; otherwise we probe the conventional bundle
    // locations so native installs (.deb/.rpm/tarball, which ship the built UI
    // under `share/xiaoguai/static`) serve it with zero config.
    if let Some(path) = resolve_static_dir(static_dir.as_deref()) {
        app = xiaoguai_api::static_ui::mount_static_ui(app, &path);
        tracing::info!(static_dir = %path.display(), "serve: web UI mounted (chat-ui at /, admin-ui at /admin)");
    } else {
        tracing::info!("serve: no web UI bundle found — serving API only");
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

    // SEC-26: security response headers on every response (API + bundled web
    // UI). The SPAs render untrusted LLM output, so CSP is the last-line
    // defence; the Vite bundles ship no inline scripts (modulepreload polyfill
    // disabled) so `script-src 'self'` does not break them.
    app = app.layer(axum::middleware::from_fn(add_security_headers));

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    let local = listener.local_addr().context("read local addr")?;
    let fut = async move { axum::serve(listener, app.into_make_service()).await };
    Ok((local, fut))
}

/// SEC-01: is `host` a loopback bind (safe to run unauthenticated)?
/// `localhost`, `127.0.0.0/8`, and `::1` are loopback; `0.0.0.0` / `::`
/// (unspecified = all interfaces) and any routable address are not. An
/// unparseable hostname is treated as non-loopback (fail-safe).
fn host_is_loopback(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// SEC-01: explicit opt-out allowing an unauthenticated non-loopback bind.
/// Off unless `XIAOGUAI_ALLOW_UNAUTHENTICATED_NONLOOPBACK` is a truthy value.
fn allow_unauthenticated_nonloopback() -> bool {
    matches!(
        std::env::var("XIAOGUAI_ALLOW_UNAUTHENTICATED_NONLOOPBACK")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Wire the single-owner auth gate (DEC-033). Returns `None` to keep the
/// open dev-mode path (handlers fall back to the owner identity); returns
/// `Some(StaticCredentialValidator)` to require a matching HTTP Basic
/// username/password on `/v1/**` when both credentials are configured.
fn build_auth(
    settings: &Settings,
) -> Option<std::sync::Arc<dyn xiaoguai_api::auth::TokenValidator>> {
    use std::sync::Arc;
    use xiaoguai_api::auth::{StaticCredentialValidator, TokenValidator};
    if !settings.auth.is_enabled() {
        tracing::warn!(
            "serve: owner auth gate DISABLED (no username/password configured) — \
             bind to localhost or set auth.username + auth.password before exposing on a URL"
        );
        return None;
    }
    let validator = StaticCredentialValidator::new(
        settings.auth.username.clone(),
        settings.auth.password.clone(),
    );
    let wrapper: Arc<dyn TokenValidator> = Arc::new(validator);
    tracing::info!(
        username = %settings.auth.username,
        "serve: owner auth gate enabled (HTTP Basic)"
    );
    Some(wrapper)
}

/// SEC-26: security headers on every response. CSP is the headline defence
/// against XSS in the SPAs that render untrusted LLM output; the rest are
/// cheap, broadly-safe hardening. `script-src 'self'` is safe because the Vite
/// bundles carry no inline scripts (modulepreload polyfill disabled);
/// `style-src` keeps `'unsafe-inline'` for React's inline styles.
async fn add_security_headers(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::{header, HeaderValue};
    const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
                       img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; \
                       object-src 'none'; base-uri 'none'; frame-ancestors 'none'";
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    resp
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
    pool: sqlx::SqlitePool,
) -> std::sync::Arc<xiaoguai_api::EvalService> {
    use std::path::PathBuf;
    use std::sync::Arc;
    use xiaoguai_api::EvalService;
    use xiaoguai_eval::{DefaultEvalAgentBuilder, EvalRunner};

    let suites_dir = PathBuf::from(settings.eval.suites_dir.clone());
    let runner = EvalRunner::new(Arc::new(DefaultEvalAgentBuilder::new(
        settings.eval.max_iterations,
    )));
    let source = Arc::new(crate::eval_bridge::SqliteCaseFromSessionSource::new(pool));
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
async fn check_mcp_oauth_keyring(pool: &sqlx::SqlitePool) -> Result<()> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mcp_oauth_tokens")
        .fetch_one(pool)
        .await
        .context("db count mcp_oauth_tokens")?;
    if count == 0 {
        tracing::debug!(
            "mcp_oauth_tokens empty; skipping keyring requirement (fresh install path)"
        );
        return Ok(());
    }
    match xiaoguai_mcp::auth::mcp_keyring_from_env() {
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

#[cfg(test)]
mod static_dir_tests {
    use super::resolve_static_dir;

    #[test]
    fn empty_string_is_explicit_opt_out() {
        assert!(resolve_static_dir(Some("")).is_none());
        assert!(resolve_static_dir(Some("   ")).is_none());
    }

    #[test]
    fn nonexistent_explicit_dir_falls_through_to_none() {
        assert!(resolve_static_dir(Some("/no/such/xiaoguai/static")).is_none());
    }

    #[test]
    fn explicit_existing_dir_is_used() {
        let dir = tempfile::tempdir().expect("tempdir");
        let resolved = resolve_static_dir(Some(dir.path().to_str().unwrap()))
            .expect("an existing explicit dir resolves");
        assert_eq!(resolved, dir.path());
    }
}
