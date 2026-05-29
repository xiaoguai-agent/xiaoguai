//! Xiaoguai CLI entry point. Subcommand bodies live in `xiaoguai_cli::commands`
//! so they remain unit-testable.

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use xiaoguai_cli::commands::{
    anomaly, audit_export, backup, chat, completions, eval, hotl, manpages, mcp, outcomes,
    provider, remote, self_update, skills, tasks, watch,
};
use xiaoguai_config::Settings;
use xiaoguai_storage::{
    connect,
    repositories::{
        LlmProviderRepository, McpServerRepository, PgLlmProviderRepository, PgMcpServerRepository,
    },
};

#[derive(Parser)]
#[command(name = "xiaoguai", version, about = "Xiaoguai CLI")]
struct Cli {
    /// Path to a YAML config file. Defaults to `~/.xiaoguai/config.yaml` if
    /// the file exists, otherwise an env-driven default.
    #[arg(long, global = true)]
    config: Option<String>,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the long-lived API server (was the `xiaoguai-core` binary).
    ///
    /// Reads the same YAML config as `xiaoguai-core` and the same
    /// `DATABASE_URL` / `XIAOGUAI_AUDIT_SIGNING_KEY` / `OLLAMA_HOST`
    /// environment. The legacy `xiaoguai-core` binary is now a thin shim
    /// over the same library entry point.
    Serve,

    /// Bootstrap-time round-trip check (PG + cache + JWT + RBAC + audit).
    ///
    /// Exits 0 on success, non-zero on any subsystem failure. Useful as
    /// a systemd `ExecStartPre=` or container healthcheck.
    Smoke,

    /// Send a one-shot prompt to the agent and print the response.
    Chat {
        /// User prompt.
        #[arg(long)]
        prompt: String,
        /// Use the deterministic mock backend (no network).
        #[arg(long, conflicts_with = "ollama_url")]
        mock: bool,
        /// Override Ollama base URL (default <http://localhost:11434>).
        #[arg(long)]
        ollama_url: Option<String>,
        /// LLM model name (default `qwen2.5-coder`).
        #[arg(long, default_value = "qwen2.5-coder")]
        model: String,
    },

    /// Administer the LLM provider registry (Postgres-backed).
    Provider {
        #[command(subcommand)]
        action: ProviderCmd,
    },

    /// Administer the MCP server registry (Postgres-backed).
    Mcp {
        #[command(subcommand)]
        action: McpCmd,
    },

    /// Talk to a running `xiaoguai-api` over HTTP/SSE.
    Remote {
        /// Base URL of the API server, e.g. `http://localhost:8080`.
        #[arg(long)]
        server: String,
        #[command(subcommand)]
        action: RemoteCmd,
    },

    /// Run an eval suite (`*.eval.yaml` cases) against the deterministic
    /// `MockBackend` substrate and print pass/fail.
    Eval {
        #[command(subcommand)]
        action: EvalCmd,
    },

    /// Write a shell completion script to stdout.
    ///
    /// Source the output in your shell init file.
    #[command(hide = true)]
    Completions {
        /// Target shell.
        shell: completions::Shell,
    },

    /// Generate man pages into a directory.
    #[command(hide = true)]
    Manpages {
        /// Output directory (created if absent). Defaults to `./man`.
        #[arg(default_value = "man")]
        outdir: String,
    },

    /// Create a backup archive (`pg_dump` + config + audit DB).
    Backup {
        /// Output path for the `.tar.gz` file.
        #[arg(long)]
        out: String,
        /// PostgreSQL connection URL.  Defaults to `$DATABASE_URL`.
        #[arg(long, env = "DATABASE_URL")]
        database_url: String,
        /// Encrypt the archive with an age public-key file.
        #[arg(long)]
        encrypt: Option<String>,
    },

    /// Restore a backup archive created by `xiaoguai backup`.
    Restore {
        /// Path to the backup `.tar.gz` (or `.tar.gz.age`) file.
        #[arg(long = "in")]
        input: String,
        /// Directory to extract into.
        #[arg(long, default_value = "./restore-out")]
        outdir: String,
        /// Overwrite existing output directory.
        #[arg(long)]
        force: bool,
        /// Age identity file for decryption (required for encrypted backups).
        #[arg(long)]
        identity: Option<String>,
    },

    /// Check for and apply binary updates from GitHub Releases.
    SelfUpdate {
        /// Only report whether an update is available; do not download.
        #[arg(long)]
        check: bool,
    },

    // ------------------------------------------------------------------
    // Wave-3 subcommands
    // ------------------------------------------------------------------
    /// Administer Human-on-the-Loop (HOTL) budget policies.
    ///
    /// Manages spend/count caps per tenant and action scope. Enforcer
    /// integration ships in v1.3; on 503 a friendly message is printed.
    Hotl {
        /// Base URL of the `xiaoguai-api` server.
        #[arg(
            long,
            env = "XIAOGUAI_API_BASE",
            default_value = "http://localhost:8080"
        )]
        api_base: String,
        /// Output format.
        #[arg(long, default_value = "table")]
        output: String,
        #[command(subcommand)]
        action: HotlCmd,
    },

    /// Manage agent outcome telemetry (ROI tracking).
    ///
    /// Records and queries business-value attributions (`revenue_usd`,
    /// `hours_saved`, etc.). Pg bridge ships in v1.3; on 503 a friendly
    /// message is printed.
    Outcomes {
        /// Base URL of the `xiaoguai-api` server.
        #[arg(
            long,
            env = "XIAOGUAI_API_BASE",
            default_value = "http://localhost:8080"
        )]
        api_base: String,
        /// Output format.
        #[arg(long, default_value = "table")]
        output: String,
        #[command(subcommand)]
        action: OutcomesCmd,
    },

    /// Manage the skill-pack marketplace.
    ///
    /// Lists catalog packs, installs or uninstalls packs for a tenant.
    /// Pg bridge ships in v1.3; on 503 a friendly message is printed.
    Skills {
        /// Base URL of the `xiaoguai-api` server.
        #[arg(
            long,
            env = "XIAOGUAI_API_BASE",
            default_value = "http://localhost:8080"
        )]
        api_base: String,
        /// Output format.
        #[arg(long, default_value = "table")]
        output: String,
        #[command(subcommand)]
        action: SkillsCmd,
    },

    /// Manage declarative active-wakeup watchers.
    ///
    /// Registers SQL/HTTP watchers that fire when query rows match or a
    /// `JSONPath` expression hits. Pg bridge ships in v1.3; on 503 a
    /// friendly message is printed.
    Watch {
        /// Base URL of the `xiaoguai-api` server.
        #[arg(
            long,
            env = "XIAOGUAI_API_BASE",
            default_value = "http://localhost:8080"
        )]
        api_base: String,
        /// Output format.
        #[arg(long, default_value = "table")]
        output: String,
        #[command(subcommand)]
        action: WatchCmd,
    },

    /// Manage time-series anomaly monitors (Z-score / EWMA).
    ///
    /// Registers anomaly specs that fire when a KPI deviates statistically.
    /// Pg bridge ships in v1.3; on 503 a friendly message is printed.
    Anomaly {
        /// Base URL of the `xiaoguai-api` server.
        #[arg(
            long,
            env = "XIAOGUAI_API_BASE",
            default_value = "http://localhost:8080"
        )]
        api_base: String,
        /// Output format.
        #[arg(long, default_value = "table")]
        output: String,
        #[command(subcommand)]
        action: AnomalyCmd,
    },

    /// Kanban task board management (v1.4-ready — requires /v1/tasks backend).
    Tasks {
        /// Base URL of the API server, e.g. `http://localhost:8080`.
        #[arg(long, global = true, default_value = "http://localhost:8080")]
        api_base: String,
        #[command(subcommand)]
        action: TasksCmd,
    },

    /// Audit chain administration — T5 compliance export.
    Audit {
        /// Base URL of the `xiaoguai-api` server.
        #[arg(
            long,
            env = "XIAOGUAI_API_BASE",
            default_value = "http://localhost:8080"
        )]
        api_base: String,
        #[command(subcommand)]
        action: AuditCmd,
    },
}

#[derive(Subcommand)]
enum AuditCmd {
    /// Export a compliance bundle (SOC2 / GDPR / HIPAA) over a time window.
    ///
    /// Chain verification runs inside the window before the bundle is
    /// rendered. If the chain is broken, the call returns 409 + a
    /// machine-readable error JSON and this command exits non-zero. There
    /// is no `--skip-verify` flag by design.
    Export {
        /// Tenant id (required — chain is per-tenant).
        #[arg(long)]
        tenant_id: String,
        /// Framework short name — `soc2` | `gdpr` | `hipaa`.
        #[arg(long)]
        framework: String,
        /// RFC3339 inclusive lower bound, e.g. `2026-01-01T00:00:00Z`.
        #[arg(long)]
        from: String,
        /// RFC3339 inclusive upper bound, e.g. `2026-04-01T00:00:00Z`.
        #[arg(long)]
        to: String,
        /// Path to write the rendered bundle to.
        #[arg(long)]
        output: String,
        /// Output format — `json` (canonical) or `csv` (auditor-friendly).
        /// `pdf` is reserved (returns 501 — tracked as a follow-up).
        #[arg(long, default_value = "json")]
        format: String,
    },
}

// ---------------------------------------------------------------------------
// Wave-3 sub-enums
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
enum HotlCmd {
    /// HOTL policy administration.
    Policy {
        #[command(subcommand)]
        action: HotlPolicyCmd,
    },
    /// Run a one-shot budget check against the live enforcer.
    Check {
        /// Tenant context.
        #[arg(long)]
        tenant_id: String,
        /// Action category (e.g. `llm_call`, `email_send`).
        #[arg(long)]
        scope: String,
        /// Simulated cost/count increment.
        #[arg(long)]
        amount: f64,
    },
}

#[derive(Subcommand)]
enum HotlPolicyCmd {
    /// Create a new HOTL budget policy.
    Create {
        /// Tenant the policy applies to.
        #[arg(long)]
        tenant_id: String,
        /// Action category.
        #[arg(long)]
        scope: String,
        /// Rolling window width in seconds.
        #[arg(long)]
        window_secs: u64,
        /// Maximum invocation count within the window.
        #[arg(long)]
        max_count: Option<u64>,
        /// Maximum cumulative USD cost within the window.
        #[arg(long)]
        max_usd: Option<f64>,
        /// IM channel / email to notify on breach; absent = deny.
        #[arg(long)]
        escalate_to: Option<String>,
    },
    /// List policies for a tenant.
    List {
        /// Tenant to filter.
        #[arg(long)]
        tenant_id: String,
        /// Further filter by action category.
        #[arg(long)]
        scope: Option<String>,
    },
    /// Fetch a single policy by id.
    Get {
        /// Policy id.
        #[arg(long)]
        id: String,
    },
    /// Update mutable fields of an existing policy.
    Update {
        /// Policy id.
        #[arg(long)]
        id: String,
        #[arg(long)]
        max_count: Option<u64>,
        #[arg(long)]
        max_usd: Option<f64>,
        #[arg(long)]
        escalate_to: Option<String>,
        #[arg(long)]
        window_secs: Option<u64>,
    },
    /// Delete a policy by id.
    Delete {
        /// Policy id.
        #[arg(long)]
        id: String,
    },
}

#[derive(Subcommand)]
enum OutcomesCmd {
    /// Record one outcome attribution.
    Record {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        agent_name: String,
        /// One of: `revenue_usd`, `cost_saved_usd`, `hours_saved`, `deals_closed`,
        /// `tickets_resolved`, custom.
        #[arg(long)]
        kind: String,
        #[arg(long)]
        value: f64,
        #[arg(long)]
        session_id: Option<String>,
        /// Unit label for `custom` kind.
        #[arg(long)]
        unit: Option<String>,
        #[arg(long)]
        description: Option<String>,
    },
    /// List raw outcome records.
    List {
        #[arg(long)]
        tenant_id: String,
        /// Time range shorthand: `24h`, `7d`, `30d` (default: `30d`).
        #[arg(long, default_value = "30d")]
        range: String,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
    /// Aggregated ROI summary.
    Summary {
        #[arg(long)]
        tenant_id: String,
        #[arg(long, default_value = "30d")]
        range: String,
    },
    /// Day-by-day breakdown of outcome values.
    Timeseries {
        #[arg(long)]
        tenant_id: String,
        #[arg(long, default_value = "30d")]
        range: String,
        #[arg(long)]
        kind: Option<String>,
    },
}

#[derive(Subcommand)]
enum SkillsCmd {
    /// List catalog packs or installed packs.
    List {
        #[arg(long)]
        tenant_id: Option<String>,
        /// Filter catalog by category.
        #[arg(long)]
        category: Option<String>,
        /// Show installed packs instead of full catalog.
        #[arg(long)]
        installed: bool,
    },
    /// Install a catalog pack for a tenant.
    Install {
        #[arg(long)]
        tenant_id: String,
        /// Catalog slug (see `xiaoguai skills list`).
        #[arg(long)]
        pack: String,
        /// Operator knob overrides as inline JSON.
        #[arg(long)]
        config: Option<String>,
    },
    /// Install a local pack definition (planned for v1.3).
    InstallFromFile {
        #[arg(long)]
        tenant_id: String,
        #[arg(long)]
        file: String,
    },
    /// Uninstall a pack by installed-row id.
    Uninstall {
        #[arg(long)]
        id: String,
    },
}

#[derive(Subcommand)]
enum WatchCmd {
    /// List registered watch specs.
    List {
        #[arg(long)]
        tenant_id: Option<String>,
    },
    /// Register and activate a watch spec from a YAML file.
    Start {
        /// Path to `WatchSpec` YAML file.
        #[arg(long)]
        file: String,
        #[arg(long)]
        tenant_id: Option<String>,
    },
    /// Deactivate a watcher by id.
    Stop {
        #[arg(long)]
        id: String,
    },
    /// Run one poll cycle and print matched rows (no side effects).
    Test {
        #[arg(long)]
        id: String,
    },
}

#[derive(Subcommand)]
enum AnomalyCmd {
    /// Register an anomaly spec from a YAML file and arm it.
    Run {
        /// Path to `AnomalySpec` YAML file.
        #[arg(long)]
        file: String,
    },
    /// Back-test a detector spec against a CSV of historical observations.
    Test {
        /// Path to `AnomalySpec` YAML file.
        #[arg(long)]
        file: String,
        /// CSV file with timestamp and value columns.
        #[arg(long)]
        data: String,
        /// Timestamp column name.
        #[arg(long, default_value = "ts")]
        ts_col: String,
        /// Value column name.
        #[arg(long, default_value = "value")]
        val_col: String,
    },
}

// ---------------------------------------------------------------------------
// Existing sub-enums (unchanged)
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
enum EvalCmd {
    /// Walk a directory of `.eval.yaml` cases and grade them.
    Run {
        /// Suite name. Becomes `EvalReport.suite`; also the default
        /// case directory under `./eval/<suite>` when `--cases-dir`
        /// is omitted.
        #[arg(long)]
        suite: String,
        /// Directory holding `*.eval.yaml` files. Flat (no recursion).
        #[arg(long)]
        cases_dir: Option<String>,
        /// Optional path to write the JSON report to. Omit for
        /// stdout-only output.
        #[arg(long)]
        out: Option<String>,
        /// Override the agent loop's `max_iterations`. `0` = use
        /// `AgentConfig::new`'s default (8).
        #[arg(long, default_value_t = 0)]
        max_iterations: u32,
    },
}

#[derive(Subcommand)]
enum RemoteCmd {
    /// Smoke test the remote server.
    Healthz,
    /// Send one prompt against a fresh session and print the streamed reply.
    Chat {
        #[arg(long)]
        user_id: String,
        #[arg(long)]
        tenant_id: String,
        #[arg(long, default_value = "qwen2.5-coder")]
        model: String,
        #[arg(long)]
        prompt: String,
        /// Optional title for the new session.
        #[arg(long)]
        title: Option<String>,
    },
    /// Fetch and print the message history of an existing session.
    Messages {
        #[arg(long)]
        session: String,
    },
    /// Cancel an in-flight agent run.
    Cancel {
        #[arg(long)]
        session: String,
    },
}

#[derive(Subcommand)]
enum McpCmd {
    /// Register a new MCP server.
    Register {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "1.0.0")]
        version: String,
        /// One of: `stdio`, `sse`, `http`.
        #[arg(long)]
        transport: String,
        /// Command to spawn (required for transport=stdio).
        #[arg(long)]
        command: Option<String>,
        /// Comma-separated args to pass to the command.
        #[arg(long, value_delimiter = ',', default_value = "")]
        args: Vec<String>,
        /// Comma-separated env-var NAMES the server needs. Values are
        /// resolved at spawn time — never stored.
        #[arg(long, value_delimiter = ',', default_value = "")]
        env_keys: Vec<String>,
        /// Endpoint URL (required for transport=sse|http).
        #[arg(long)]
        endpoint: Option<String>,
        /// Tenant id for tenant-scoped servers. Omit for system-wide.
        #[arg(long)]
        tenant: Option<String>,
    },
    /// List MCP servers (omit `--tenant` for globals only).
    List {
        #[arg(long)]
        tenant: Option<String>,
    },
    /// Remove an MCP server by id.
    Remove {
        #[arg(long)]
        id: String,
    },
}

#[derive(Subcommand)]
enum ProviderCmd {
    /// Register a new provider. `--api-key-env` names an env var; the key
    /// itself is never written to the database.
    Register {
        #[arg(long)]
        name: String,
        /// One of: `ollama`, `openai_compat`, `anthropic`, `gemini`, `bedrock`,
        /// `azure_openai`, `mistral`, `groq`.
        #[arg(long)]
        kind: String,
        #[arg(long)]
        endpoint: String,
        /// Comma-separated list of supported model names.
        #[arg(long, value_delimiter = ',')]
        models: Vec<String>,
        /// Comma-separated list of models this provider should be the
        /// default for (within its scope).
        #[arg(long, value_delimiter = ',', default_value = "")]
        default_for: Vec<String>,
        #[arg(long, default_value_t = 100)]
        fallback_order: i32,
        /// Name of the env var holding the API key.
        #[arg(long)]
        api_key_env: Option<String>,
        /// Tenant id for tenant-scoped providers. Omit for system-wide.
        #[arg(long)]
        tenant: Option<String>,
    },
    /// List providers in a given scope (omit `--tenant` for globals only).
    List {
        #[arg(long)]
        tenant: Option<String>,
    },
    /// Remove a provider by id.
    Remove {
        #[arg(long)]
        id: String,
    },
}

#[derive(Subcommand)]
enum TasksCmd {
    /// List tasks on a board, optionally filtered by column.
    List {
        /// Board name to query.
        #[arg(long, default_value = "default")]
        board: String,
        /// Filter by column name (e.g. `triage`, `running`, `done`).
        #[arg(long)]
        column: Option<String>,
    },
    /// Create a new task on a board.
    Create {
        /// Task title (required).
        #[arg(long)]
        title: String,
        /// Optional description.
        #[arg(long)]
        description: Option<String>,
        /// Target board.
        #[arg(long, default_value = "default")]
        board: String,
        /// Initial column for the task.
        #[arg(long, default_value = "triage")]
        column: String,
    },
    /// Move a task to another column.
    Move {
        /// Task identifier.
        task_id: String,
        /// Destination column name.
        #[arg(long)]
        to: String,
    },
    /// Claim a task (transition to RUNNING and assign to an agent).
    Claim {
        /// Task identifier.
        task_id: String,
        /// Agent name or identifier. Defaults to the current process name.
        #[arg(long)]
        agent: Option<String>,
    },
    /// Mark a task as complete.
    Complete {
        /// Task identifier.
        task_id: String,
        /// Optional outcome summary stored alongside the task.
        #[arg(long)]
        outcome: Option<String>,
    },
    /// Block a task with a human-readable reason.
    Block {
        /// Task identifier.
        task_id: String,
        /// Reason the task is blocked.
        #[arg(long)]
        reason: String,
    },
    /// Dispatch the next READY card(s) to RUNNING (pool-worker pattern).
    Dispatch {
        /// Board to pull from.
        #[arg(long, default_value = "default")]
        board: String,
        /// Number of tasks to dispatch in one call.
        #[arg(long, default_value_t = 1)]
        n: usize,
    },
    /// Show task detail and history.
    Show {
        /// Task identifier.
        task_id: String,
    },
}

// ---------------------------------------------------------------------------
// Handlers — existing
// ---------------------------------------------------------------------------

fn load_settings(config: Option<&str>) -> Result<Settings> {
    match config {
        Some(path) => {
            Settings::load_from_file(path).map_err(|e| anyhow::anyhow!("load config: {e}"))
        }
        None => Settings::load_from_env().map_err(|e| anyhow::anyhow!("load env config: {e}")),
    }
}

async fn build_provider_repo(config: Option<&str>) -> Result<PgLlmProviderRepository> {
    let settings = load_settings(config)?;
    let pool = connect(&settings.database.url, settings.database.max_connections)
        .await
        .context("connect to postgres")?;
    Ok(PgLlmProviderRepository::new(pool))
}

async fn build_mcp_repo(config: Option<&str>) -> Result<PgMcpServerRepository> {
    let settings = load_settings(config)?;
    let pool = connect(&settings.database.url, settings.database.max_connections)
        .await
        .context("connect to postgres")?;
    Ok(PgMcpServerRepository::new(pool))
}

async fn handle_chat(
    prompt: String,
    mock: bool,
    ollama_url: Option<String>,
    model: String,
) -> Result<()> {
    let answer = chat::run(chat::ChatArgs {
        prompt,
        mock,
        ollama_url,
        model,
    })
    .await?;
    println!("{answer}");
    Ok(())
}

async fn handle_provider(config: Option<&str>, action: ProviderCmd) -> Result<()> {
    let repo = build_provider_repo(config).await?;
    let repo: &dyn LlmProviderRepository = &repo;
    match action {
        ProviderCmd::Register {
            name,
            kind,
            endpoint,
            models,
            default_for,
            fallback_order,
            api_key_env,
            tenant,
        } => {
            let p = provider::register(
                repo,
                provider::RegisterArgs {
                    name,
                    kind,
                    endpoint,
                    models,
                    default_for: default_for.into_iter().filter(|s| !s.is_empty()).collect(),
                    fallback_order,
                    api_key_env,
                    tenant,
                },
            )
            .await?;
            println!("registered {} ({})", p.id, p.name);
        }
        ProviderCmd::List { tenant } => {
            let rows = provider::list(repo, provider::ListArgs { tenant }).await?;
            print!("{}", provider::format_table(&rows));
        }
        ProviderCmd::Remove { id } => {
            provider::remove(repo, provider::RemoveArgs { id: id.clone() }).await?;
            println!("removed {id}");
        }
    }
    Ok(())
}

async fn handle_mcp(config: Option<&str>, action: McpCmd) -> Result<()> {
    let repo = build_mcp_repo(config).await?;
    let repo: &dyn McpServerRepository = &repo;
    match action {
        McpCmd::Register {
            name,
            version,
            transport,
            command,
            args,
            env_keys,
            endpoint,
            tenant,
        } => {
            let server = mcp::register(
                repo,
                mcp::RegisterArgs {
                    name,
                    version,
                    transport,
                    command,
                    args: args.into_iter().filter(|s| !s.is_empty()).collect(),
                    env_keys: env_keys.into_iter().filter(|s| !s.is_empty()).collect(),
                    endpoint,
                    tenant,
                },
            )
            .await?;
            println!(
                "registered {} ({}@{})",
                server.id, server.name, server.version
            );
        }
        McpCmd::List { tenant } => {
            let rows = mcp::list(repo, mcp::ListArgs { tenant }).await?;
            print!("{}", mcp::format_table(&rows));
        }
        McpCmd::Remove { id } => {
            mcp::remove(repo, mcp::RemoveArgs { id: id.clone() }).await?;
            println!("removed {id}");
        }
    }
    Ok(())
}

async fn handle_remote(server: String, action: RemoteCmd) -> Result<()> {
    use std::io::Write;
    let client = remote::RemoteClient::new(server);
    match action {
        RemoteCmd::Healthz => {
            let body = client.healthz().await?;
            println!("{body}");
        }
        RemoteCmd::Chat {
            user_id,
            tenant_id,
            model,
            prompt,
            title,
        } => {
            let session = client
                .create_session(&remote::CreateSessionRequest {
                    user_id,
                    tenant_id,
                    model,
                    title,
                })
                .await?;
            eprintln!("session: {}", session.id);
            client
                .send_message(&session.id, &prompt, |ev| {
                    match ev.name.as_str() {
                        "text_delta" => {
                            if let Some(delta) =
                                ev.payload.get("delta").and_then(serde_json::Value::as_str)
                            {
                                print!("{delta}");
                                std::io::stdout().flush().ok();
                            }
                        }
                        "tool_call_started" => {
                            let name = ev
                                .payload
                                .get("name")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("?");
                            eprintln!("\n[tool start] {name}");
                        }
                        "tool_call_finished" => {
                            let ok = ev
                                .payload
                                .get("ok")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false);
                            eprintln!("[tool finish] ok={ok}");
                        }
                        "done" => {
                            println!();
                            let reason = ev
                                .payload
                                .get("stop_reason")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("?");
                            eprintln!("[done] {reason}");
                        }
                        "error" => {
                            let msg = ev
                                .payload
                                .get("message")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("?");
                            eprintln!("[error] {msg}");
                        }
                        _ => {}
                    }
                    Ok(())
                })
                .await?;
        }
        RemoteCmd::Messages { session } => {
            let msgs = client.list_messages(&session).await?;
            println!("{}", serde_json::to_string_pretty(&msgs)?);
        }
        RemoteCmd::Cancel { session } => {
            let cancelled = client.cancel(&session).await?;
            println!("cancelled={cancelled}");
        }
    }
    Ok(())
}

async fn handle_eval(action: EvalCmd) -> Result<()> {
    match action {
        EvalCmd::Run {
            suite,
            cases_dir,
            out,
            max_iterations,
        } => {
            let report = eval::run(eval::EvalArgs {
                suite,
                cases_dir: cases_dir.map(std::path::PathBuf::from),
                out: out.map(std::path::PathBuf::from),
                max_iterations,
            })
            .await?;
            print!("{}", xiaoguai_eval::pretty_summary(&report));
            if report.failed() > 0 {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers — wave-3
// ---------------------------------------------------------------------------

async fn handle_hotl(api_base: String, output: String, action: HotlCmd) -> Result<()> {
    match action {
        HotlCmd::Policy { action } => match action {
            HotlPolicyCmd::Create {
                tenant_id,
                scope,
                window_secs,
                max_count,
                max_usd,
                escalate_to,
            } => {
                let v = hotl::policy_create(hotl::PolicyCreateArgs {
                    api_base,
                    tenant_id,
                    scope,
                    window_secs,
                    max_count,
                    max_usd,
                    escalate_to,
                })
                .await?;
                print_value(&v, &output)?;
            }
            HotlPolicyCmd::List { tenant_id, scope } => {
                let rows = hotl::policy_list(hotl::PolicyListArgs {
                    api_base,
                    tenant_id,
                    scope,
                })
                .await?;
                if output == "table" {
                    print!("{}", hotl::format_policy_table(&rows));
                } else {
                    print_value(&serde_json::to_value(&rows)?, &output)?;
                }
            }
            HotlPolicyCmd::Get { id } => {
                let v = hotl::policy_get(hotl::PolicyGetArgs { api_base, id }).await?;
                print_value(&v, &output)?;
            }
            HotlPolicyCmd::Update {
                id,
                max_count,
                max_usd,
                escalate_to,
                window_secs,
            } => {
                let v = hotl::policy_update(hotl::PolicyUpdateArgs {
                    api_base,
                    id,
                    max_count,
                    max_usd,
                    escalate_to,
                    window_secs,
                })
                .await?;
                print_value(&v, &output)?;
            }
            HotlPolicyCmd::Delete { id } => {
                let id_clone = id.clone();
                hotl::policy_delete(hotl::PolicyDeleteArgs { api_base, id }).await?;
                println!("deleted {id_clone}");
            }
        },
        HotlCmd::Check {
            tenant_id,
            scope,
            amount,
        } => {
            let resp = hotl::check(hotl::CheckArgs {
                api_base,
                tenant_id,
                scope,
                amount,
            })
            .await?;
            println!("verdict: {}", resp.verdict);
            if let Some(reason) = resp.reason {
                println!("reason: {reason:?}");
            }
        }
    }
    Ok(())
}

async fn handle_outcomes(api_base: String, output: String, action: OutcomesCmd) -> Result<()> {
    match action {
        OutcomesCmd::Record {
            tenant_id,
            agent_name,
            kind,
            value,
            session_id,
            unit,
            description,
        } => {
            let v = outcomes::record(outcomes::RecordArgs {
                api_base,
                tenant_id,
                agent_name,
                kind,
                value,
                session_id,
                unit,
                description,
            })
            .await?;
            print_value(&v, &output)?;
        }
        OutcomesCmd::List {
            tenant_id,
            range,
            kind,
            limit,
        } => {
            let rows = outcomes::list(outcomes::ListArgs {
                api_base,
                tenant_id,
                range,
                kind,
                limit,
            })
            .await?;
            if output == "table" {
                print!("{}", outcomes::format_list_table(&rows));
            } else {
                print_value(&serde_json::to_value(&rows)?, &output)?;
            }
        }
        OutcomesCmd::Summary { tenant_id, range } => {
            let rows = outcomes::summary(outcomes::SummaryArgs {
                api_base,
                tenant_id,
                range,
            })
            .await?;
            if output == "table" {
                print!("{}", outcomes::format_summary_table(&rows));
            } else {
                print_value(&serde_json::to_value(&rows)?, &output)?;
            }
        }
        OutcomesCmd::Timeseries {
            tenant_id,
            range,
            kind,
        } => {
            let v = outcomes::timeseries(outcomes::TimeseriesArgs {
                api_base,
                tenant_id,
                range,
                kind,
            })
            .await?;
            print_value(&v, &output)?;
        }
    }
    Ok(())
}

async fn handle_skills(api_base: String, output: String, action: SkillsCmd) -> Result<()> {
    match action {
        SkillsCmd::List {
            tenant_id,
            category,
            installed,
        } => {
            let rows = skills::list(skills::ListArgs {
                api_base,
                tenant_id,
                category,
                installed,
            })
            .await?;
            if output == "table" {
                if installed {
                    print!("{}", skills::format_installed_table(&rows));
                } else {
                    print!("{}", skills::format_catalog_table(&rows));
                }
            } else {
                print_value(&serde_json::to_value(&rows)?, &output)?;
            }
        }
        SkillsCmd::Install {
            tenant_id,
            pack,
            config,
        } => {
            let v = skills::install(skills::InstallArgs {
                api_base,
                tenant_id,
                pack,
                config,
            })
            .await?;
            print_value(&v, &output)?;
        }
        SkillsCmd::InstallFromFile { .. } => {
            skills::install_from_file_not_implemented()?;
        }
        SkillsCmd::Uninstall { id } => {
            skills::uninstall(skills::UninstallArgs {
                api_base,
                id: id.clone(),
            })
            .await?;
            println!("{}", serde_json::json!({"ok": true}));
        }
    }
    Ok(())
}

async fn handle_watch(api_base: String, output: String, action: WatchCmd) -> Result<()> {
    match action {
        WatchCmd::List { tenant_id } => {
            let rows = watch::list(watch::ListArgs {
                api_base,
                tenant_id,
            })
            .await?;
            if output == "table" {
                print!("{}", watch::format_list_table(&rows));
            } else {
                print_value(&serde_json::to_value(&rows)?, &output)?;
            }
        }
        WatchCmd::Start { file, tenant_id } => {
            let v = watch::start(watch::StartArgs {
                api_base,
                file: std::path::PathBuf::from(file),
                tenant_id,
            })
            .await?;
            print_value(&v, &output)?;
        }
        WatchCmd::Stop { id } => {
            let id_clone = id.clone();
            watch::stop(watch::StopArgs { api_base, id }).await?;
            println!("stopped: {id_clone}");
        }
        WatchCmd::Test { id } => {
            let v = watch::test(watch::TestArgs { api_base, id }).await?;
            print_value(&v, &output)?;
        }
    }
    Ok(())
}

async fn handle_anomaly(api_base: String, output: String, action: AnomalyCmd) -> Result<()> {
    match action {
        AnomalyCmd::Run { file } => {
            let v = anomaly::run(anomaly::RunArgs {
                api_base,
                file: std::path::PathBuf::from(file),
            })
            .await?;
            print_value(&v, &output)?;
        }
        AnomalyCmd::Test {
            file,
            data,
            ts_col,
            val_col,
        } => {
            let result = anomaly::backtest(anomaly::BacktestArgs {
                api_base,
                file: std::path::PathBuf::from(file),
                data: std::path::PathBuf::from(data),
                ts_col,
                val_col,
            })
            .await?;
            if output == "table" {
                print!("{}", anomaly::format_backtest_table(&result));
            } else {
                print_value(&result, &output)?;
            }
        }
    }
    Ok(())
}

async fn handle_tasks(api_base: String, action: TasksCmd) -> Result<()> {
    let client = tasks::TasksClient::new(api_base);
    match action {
        TasksCmd::List { board, column } => {
            let result = client.list(&board, column.as_deref()).await;
            match result {
                Ok(v) => println!("{}", tasks::pretty(&v)),
                Err(e) => println!("{e}"),
            }
        }
        TasksCmd::Create {
            title,
            description,
            board,
            column,
        } => {
            let req = tasks::CreateTaskRequest {
                title,
                description,
                board,
                column,
            };
            let result = client.create(&req).await;
            match result {
                Ok(v) => println!("{}", tasks::pretty(&v)),
                Err(e) => println!("{e}"),
            }
        }
        TasksCmd::Move { task_id, to } => {
            let result = client.move_task(&task_id, &to).await;
            match result {
                Ok(v) => println!("{}", tasks::pretty(&v)),
                Err(e) => println!("{e}"),
            }
        }
        TasksCmd::Claim { task_id, agent } => {
            let result = client.claim(&task_id, agent.as_deref()).await;
            match result {
                Ok(v) => println!("{}", tasks::pretty(&v)),
                Err(e) => println!("{e}"),
            }
        }
        TasksCmd::Complete { task_id, outcome } => {
            let result = client.complete(&task_id, outcome.as_deref()).await;
            match result {
                Ok(v) => println!("{}", tasks::pretty(&v)),
                Err(e) => println!("{e}"),
            }
        }
        TasksCmd::Block { task_id, reason } => {
            let result = client.block(&task_id, &reason).await;
            match result {
                Ok(v) => println!("{}", tasks::pretty(&v)),
                Err(e) => println!("{e}"),
            }
        }
        TasksCmd::Dispatch { board, n } => {
            let result = client.dispatch(&board, n).await;
            match result {
                Ok(v) => {
                    // Server may return empty array when no READY cards exist.
                    if v.as_array().is_some_and(Vec::is_empty) {
                        println!("no ready cards on board '{board}'");
                    } else {
                        println!("{}", tasks::pretty(&v));
                    }
                }
                Err(e) => println!("{e}"),
            }
        }
        TasksCmd::Show { task_id } => {
            let result = client.show(&task_id).await;
            match result {
                Ok(v) => println!("{}", tasks::pretty(&v)),
                Err(e) => println!("{e}"),
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Output helper
// ---------------------------------------------------------------------------

fn print_value(v: &serde_json::Value, format: &str) -> Result<()> {
    match format {
        "json" => println!("{}", serde_json::to_string_pretty(v)?),
        "yaml" => print!("{}", serde_yaml::to_string(v)?),
        _ => println!("{}", serde_json::to_string_pretty(v)?),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    let cfg = cli.config.as_deref();
    match cli.command {
        Cmd::Serve => {
            let settings = xiaoguai_core::load_settings(cfg.map(std::path::Path::new))
                .context("load settings for serve")?;
            xiaoguai_core::run_serve(&settings).await
        }
        Cmd::Smoke => {
            let settings = xiaoguai_core::load_settings(cfg.map(std::path::Path::new))
                .context("load settings for smoke")?;
            xiaoguai_core::run_smoke(&settings).await
        }
        Cmd::Chat {
            prompt,
            mock,
            ollama_url,
            model,
        } => handle_chat(prompt, mock, ollama_url, model).await,
        Cmd::Provider { action } => handle_provider(cfg, action).await,
        Cmd::Mcp { action } => handle_mcp(cfg, action).await,
        Cmd::Remote { server, action } => handle_remote(server, action).await,
        Cmd::Eval { action } => handle_eval(action).await,
        Cmd::Completions { shell } => {
            let mut cmd = Cli::command();
            completions::run(shell, &mut cmd, &mut std::io::stdout())
        }
        Cmd::Manpages { outdir } => {
            let mut cmd = Cli::command();
            let written = manpages::run(&mut cmd, std::path::Path::new(&outdir))?;
            for p in written {
                println!("wrote {}", p.display());
            }
            Ok(())
        }
        Cmd::Backup {
            out,
            database_url,
            encrypt,
        } => {
            let out_path = backup::run_backup(backup::BackupArgs {
                out: std::path::PathBuf::from(out),
                database_url,
                encrypt: encrypt.map(std::path::PathBuf::from),
            })?;
            println!("backup written to {}", out_path.display());
            Ok(())
        }
        Cmd::Restore {
            input,
            outdir,
            force,
            identity,
        } => {
            backup::run_restore(backup::RestoreArgs {
                input: std::path::PathBuf::from(input),
                outdir: std::path::PathBuf::from(outdir),
                force,
                identity: identity.map(std::path::PathBuf::from),
            })?;
            println!("restore complete");
            Ok(())
        }
        Cmd::SelfUpdate { check } => {
            self_update::run_self_update(self_update::SelfUpdateArgs {
                check,
                api_url: None,
            })
            .await
        }
        // Wave-3
        Cmd::Hotl {
            api_base,
            output,
            action,
        } => handle_hotl(api_base, output, action).await,
        Cmd::Outcomes {
            api_base,
            output,
            action,
        } => handle_outcomes(api_base, output, action).await,
        Cmd::Skills {
            api_base,
            output,
            action,
        } => handle_skills(api_base, output, action).await,
        Cmd::Watch {
            api_base,
            output,
            action,
        } => handle_watch(api_base, output, action).await,
        Cmd::Anomaly {
            api_base,
            output,
            action,
        } => handle_anomaly(api_base, output, action).await,
        // v1.4 — Kanban task board
        Cmd::Tasks { api_base, action } => handle_tasks(api_base, action).await,
        // T5 (Tier-3) — compliance export.
        Cmd::Audit { api_base, action } => handle_audit(api_base, action).await,
    }
}

async fn handle_audit(api_base: String, action: AuditCmd) -> Result<()> {
    match action {
        AuditCmd::Export {
            tenant_id,
            framework,
            from,
            to,
            output,
            format,
        } => {
            audit_export::run(audit_export::ExportArgs {
                api_base,
                tenant_id,
                framework,
                from,
                to,
                output: std::path::PathBuf::from(output),
                format,
            })
            .await
        }
    }
}
