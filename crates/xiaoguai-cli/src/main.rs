//! Xiaoguai CLI entry point. Subcommand bodies live in `xiaoguai_cli::commands`
//! so they remain unit-testable.

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use xiaoguai_cli::commands::{
    anomaly, audit_bundle, audit_export, backup, chat, code, completions, eval, hotl, init,
    manpages, mcp, outcomes, provider, remote, self_update, skills, stats, tasks, watch,
};
use xiaoguai_config::Settings;
use xiaoguai_storage::{
    connect,
    repositories::{
        LlmProviderRepository, McpServerRepository, SqliteLlmProviderRepository,
        SqliteMcpServerRepository,
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

    /// Serve the Agent Client Protocol (ACP) over stdio for IDE integration.
    ///
    /// Speaks newline-delimited JSON-RPC on stdin/stdout (logs go to stderr);
    /// point an ACP-capable editor at `xiaoguai acp` as the agent command. It
    /// drives the same governed agent loop as the server (DEC-038), reading the
    /// same config + `OLLAMA_HOST` environment, under the owner's implicit
    /// authority.
    Acp,

    /// Interactive chat REPL against a running server. Multi-turn (keeps the
    /// session's history) and uses your registered providers (`MiniMax`, etc.).
    /// Reads prompts from stdin; `/exit` or Ctrl-D quits.
    Repl {
        /// Base URL of the API server.
        #[arg(long, default_value = "http://localhost:7600")]
        server: String,
        /// User id for the session.
        #[arg(long, default_value = "usr_dev")]
        user_id: String,
        /// Model to use. Empty (the default) uses the server's default model.
        #[arg(long, default_value = "")]
        model: String,
    },

    /// Interactive setup wizard: pick a provider, enter its API key (hidden),
    /// and optionally make it the default model. Writes to the local DB; no web
    /// UI needed. Restart `xiaoguai serve` afterwards.
    Init,

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

    /// Administer the LLM provider registry (`SQLite`-backed).
    Provider {
        #[command(subcommand)]
        action: ProviderCmd,
    },

    /// Administer the MCP server registry (`SQLite`-backed).
    Mcp {
        #[command(subcommand)]
        action: McpCmd,
    },

    /// Talk to a running `xiaoguai-api` over HTTP/SSE.
    Remote {
        /// Base URL of the API server, e.g. `http://localhost:7600`.
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

    /// Create a backup archive (`SQLite` snapshot + config + audit DB).
    Backup {
        /// Output path for the `.tar.gz` file.
        #[arg(long)]
        out: String,
        /// Configured `database.url`. Empty (the default) resolves to the
        /// single-user store at `~/.xiaoguai/data.db`.
        #[arg(long, env = "DATABASE_URL", default_value = "")]
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
        /// Directory to extract the full archive into.
        #[arg(long, default_value = "./restore-out")]
        outdir: String,
        /// Overwrite an existing output directory (and the live store when
        /// `--restore-db` is given).
        #[arg(long)]
        force: bool,
        /// Age identity file for decryption (required for encrypted backups).
        #[arg(long)]
        identity: Option<String>,
        /// Also restore the archived `data.db` into the live `SQLite` store
        /// (the existing file is saved as `<path>.bak` first). Empty string =
        /// the default `~/.xiaoguai/data.db`.
        #[arg(long, num_args = 0..=1, default_missing_value = "")]
        restore_db: Option<String>,
    },

    /// Check for and apply binary updates from GitHub Releases.
    SelfUpdate {
        /// Only report whether an update is available; do not download.
        #[arg(long)]
        check: bool,
    },

    /// Summarise local LLM token usage and estimated cost.
    ///
    /// Reads the `token_usage` ledger in the `SQLite` store, joined to provider
    /// cost rates. Cost is `—` when the provider has no rate configured.
    Stats {
        /// Group-by dimension: `model` (default), `day`, or `session`.
        #[arg(long, default_value = "model")]
        by: String,
        /// Inclusive lower bound on `ts` (e.g. `2026-06-01`).
        #[arg(long)]
        since: Option<String>,
        /// Inclusive upper bound on `ts` (e.g. `2026-06-30`).
        #[arg(long)]
        until: Option<String>,
        /// Emit JSON instead of a text table.
        #[arg(long)]
        json: bool,
    },

    /// Governed coding workflow — workspace edits with checkpoint + audit.
    ///
    /// Every mutation is checkpointed and signed into the HMAC audit chain
    /// (`code.edit` / `git.commit` / `code.rollback`) carrying the workspace +
    /// checkpoint id, so a local change is as auditable + reversible as an
    /// agent-driven one (DEC-034/035). Runs under the owner's implicit
    /// authority (allow-all gate); the interactive `HotL` approve flow is the
    /// chat/server path.
    Code {
        /// Coding workspace (a git work tree; created + `git init`ed if absent).
        #[arg(long, default_value = ".")]
        workspace: String,
        #[command(subcommand)]
        action: CodeCmd,
    },

    // ------------------------------------------------------------------
    // Wave-3 subcommands
    // ------------------------------------------------------------------
    /// Administer Human-on-the-Loop (HOTL) budget policies.
    ///
    /// Manages spend/count caps per action scope. Enforcer
    /// integration ships in v1.3; on 503 a friendly message is printed.
    Hotl {
        /// Base URL of the `xiaoguai-api` server.
        #[arg(
            long,
            env = "XIAOGUAI_API_BASE",
            default_value = "http://localhost:7600"
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
            default_value = "http://localhost:7600"
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
    /// Lists catalog packs, installs or uninstalls packs.
    /// Pg bridge ships in v1.3; on 503 a friendly message is printed.
    Skills {
        /// Base URL of the `xiaoguai-api` server.
        #[arg(
            long,
            env = "XIAOGUAI_API_BASE",
            default_value = "http://localhost:7600"
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
            default_value = "http://localhost:7600"
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
            default_value = "http://localhost:7600"
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
        /// Base URL of the API server, e.g. `http://localhost:7600`.
        #[arg(long, global = true, default_value = "http://localhost:7600")]
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
            default_value = "http://localhost:7600"
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

    /// Build an evidence bundle: the chain-verified JSON export plus a
    /// human-readable Markdown transcript, in one folder (DEC-037).
    ///
    /// Same non-bypassable chain verification as `export`; a broken chain
    /// refuses the bundle.
    Bundle {
        /// Framework short name — `soc2` | `gdpr` | `hipaa`.
        #[arg(long)]
        framework: String,
        /// RFC3339 inclusive lower bound.
        #[arg(long)]
        from: String,
        /// RFC3339 inclusive upper bound.
        #[arg(long)]
        to: String,
        /// Output directory (created if absent) for `audit-bundle.json` +
        /// `transcript.md`.
        #[arg(long, default_value = "./audit-bundle")]
        out: String,
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
    /// List policies.
    List {
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
        #[arg(long, default_value = "30d")]
        range: String,
    },
    /// Day-by-day breakdown of outcome values.
    Timeseries {
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
        /// Filter catalog by category.
        #[arg(long)]
        category: Option<String>,
        /// Show installed packs instead of full catalog.
        #[arg(long)]
        installed: bool,
    },
    /// Install a catalog pack.
    Install {
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
        file: String,
    },
    /// Uninstall a pack by installed-row id.
    Uninstall {
        #[arg(long)]
        id: String,
    },
    /// Manage agent-authored skill proposals (Tier-2 D.1).
    Proposals {
        #[command(subcommand)]
        action: ProposalsCmd,
    },
}

#[derive(Subcommand)]
enum ProposalsCmd {
    /// List proposals, optionally filtered by status.
    List {
        /// One of: pending, approved, rejected, installed.
        #[arg(long)]
        status: Option<String>,
    },
    /// Approve a proposal — server writes the YAML manifest to ~/.xiaoguai/skills/.
    Approve {
        #[arg(long)]
        id: String,
        /// Identity of the approver (recorded in the audit log).
        #[arg(long)]
        decided_by: String,
    },
    /// Reject a proposal with a human-readable reason.
    Reject {
        #[arg(long)]
        id: String,
        #[arg(long)]
        decided_by: String,
        #[arg(long)]
        reason: String,
    },
}

#[derive(Subcommand)]
enum WatchCmd {
    /// List registered watch specs.
    List,
    /// Register and activate a watch spec from a YAML file.
    Start {
        /// Path to `WatchSpec` YAML file.
        #[arg(long)]
        file: String,
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
        /// Model to use. Empty (the default) lets the server pick its default
        /// model — the primary (lowest `fallback_order`) provider's first model.
        #[arg(long, default_value = "")]
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
// Tier-3 T4 added enough OAuth flags to the `Register` variant that
// `clippy::large_enum_variant` fires. Boxing the strings would make
// the clap derive output noisier than the warning warrants — clap
// allocates these once at parse time and they live for the program
// lifetime.
#[allow(clippy::large_enum_variant)]
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
        // ---- Tier-3 T4: OAuth 2.1 PKCE ----
        /// Auth method. Currently only `oauth2-pkce` is recognised;
        /// omit for static or no-auth servers.
        #[arg(long)]
        auth: Option<String>,
        /// OAuth `/authorize` endpoint (required for `--auth=oauth2-pkce`).
        #[arg(long)]
        auth_url: Option<String>,
        /// OAuth `/token` endpoint (required for `--auth=oauth2-pkce`).
        #[arg(long)]
        token_url: Option<String>,
        /// OAuth client id (required for `--auth=oauth2-pkce`).
        #[arg(long)]
        client_id: Option<String>,
        /// Comma-separated OAuth scopes.
        #[arg(long, value_delimiter = ',', default_value = "")]
        scopes: Vec<String>,
    },
    /// List MCP servers.
    List,
    /// Remove an MCP server by id.
    Remove {
        #[arg(long)]
        id: String,
    },
}

#[derive(Subcommand)]
enum ProviderCmd {
    /// Register a new provider. Give the key with `--api-key-env` (names an env
    /// var; key stays out of the DB) or `--api-key-stdin` (key read from stdin
    /// and stored in the local DB — for headless/pip installs with no web UI).
    Register {
        #[arg(long)]
        name: String,
        /// One of: `ollama`, `openai_compat`, `anthropic`, `gemini`, `bedrock`,
        /// `azure_openai`, `mistral`, `groq`, `minimax`.
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
        #[arg(long, conflicts_with = "api_key_stdin")]
        api_key_env: Option<String>,
        /// Read the API key from stdin and store it in the local DB (never
        /// argv/shell-history). Use for headless / pip installs.
        #[arg(long, conflicts_with = "api_key_env")]
        api_key_stdin: bool,
    },
    /// Update mutable fields of an existing provider, matched by `--id`. Only
    /// the flags you pass are changed. Use `--api-key-stdin` to (re)set the key.
    Update {
        #[arg(long)]
        id: String,
        #[arg(long)]
        endpoint: Option<String>,
        /// Comma-separated; replaces the model list when given.
        #[arg(long)]
        models: Option<String>,
        /// Comma-separated; replaces the default-for list (empty string clears
        /// it, making the provider opt-in / non-default).
        #[arg(long)]
        default_for: Option<String>,
        #[arg(long)]
        fallback_order: Option<i32>,
        #[arg(long, conflicts_with = "api_key_stdin")]
        api_key_env: Option<String>,
        /// Read a new API key from stdin and store it in the local DB.
        #[arg(long, conflicts_with = "api_key_env")]
        api_key_stdin: bool,
    },
    /// List providers.
    List,
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

#[derive(Subcommand)]
enum CodeCmd {
    /// Show the workspace's porcelain status.
    Status,
    /// Governed whole-file write (checkpoint → write → audit `code.edit`).
    Write {
        /// File path relative to the workspace root.
        path: String,
        /// New file contents.
        #[arg(long)]
        content: String,
    },
    /// Governed commit of all changes (checkpoint → commit → audit `git.commit`).
    Commit {
        /// Commit message.
        message: String,
    },
    /// Roll the workspace back to a checkpoint (audit `code.rollback`).
    Rollback {
        /// Checkpoint id (a commit SHA printed by `write`/`commit`).
        checkpoint: String,
    },
    /// Push a branch to a remote (egress; audit `git.push`).
    Push {
        /// Branch to push.
        branch: String,
        /// Remote name.
        #[arg(long, default_value = "origin")]
        remote: String,
    },
    /// Open a pull request via the `gh` CLI (egress; audit `pr.open`).
    OpenPr {
        /// PR title.
        title: String,
        /// PR body.
        #[arg(long, default_value = "")]
        body: String,
        /// Base branch.
        #[arg(long, default_value = "main")]
        base: String,
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

async fn build_provider_repo(config: Option<&str>) -> Result<SqliteLlmProviderRepository> {
    let settings = load_settings(config)?;
    let pool = connect(&settings.database.url, settings.database.max_connections)
        .await
        .context("open SQLite store")?;
    Ok(SqliteLlmProviderRepository::new(pool))
}

async fn build_mcp_repo(config: Option<&str>) -> Result<SqliteMcpServerRepository> {
    let settings = load_settings(config)?;
    let pool = connect(&settings.database.url, settings.database.max_connections)
        .await
        .context("open SQLite store")?;
    Ok(SqliteMcpServerRepository::new(pool))
}

async fn handle_stats(
    config: Option<&str>,
    by: String,
    since: Option<String>,
    until: Option<String>,
    json: bool,
) -> Result<()> {
    let group_by = stats::GroupBy::parse(&by)?;
    let settings = load_settings(config)?;
    let pool = connect(&settings.database.url, settings.database.max_connections)
        .await
        .context("open SQLite store")?;
    let rows = stats::query(
        &pool,
        &stats::StatsArgs {
            by: group_by,
            since,
            until,
        },
    )
    .await?;
    if rows.is_empty() {
        println!("no usage recorded yet");
        return Ok(());
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&stats::to_json(&rows))?);
    } else {
        print!("{}", stats::format_table(&rows, group_by));
    }
    Ok(())
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

/// Read an API key from stdin (consumes to EOF, trims). Backs `--api-key-stdin`
/// so the key never lands in argv or shell history. Intended for piping, e.g.
/// `printf %s "$KEY" | xiaoguai provider register --api-key-stdin ...`.
fn read_api_key_from_stdin() -> Result<String> {
    use std::io::Read as _;
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| anyhow::anyhow!("failed to read API key from stdin: {e}"))?;
    let key = buf.trim().to_string();
    if key.is_empty() {
        return Err(anyhow::anyhow!(
            "--api-key-stdin was set but stdin was empty; pipe the key, e.g. \
             `printf %s \"$KEY\" | xiaoguai provider register --api-key-stdin ...`"
        ));
    }
    Ok(key)
}

/// Split a comma-separated CLI value into a clean list (drops empty segments).
fn split_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// Read one line of input from stdin (visible).
fn prompt_line() -> Result<String> {
    let mut s = String::new();
    // `read_line` returns Ok(0) at EOF (closed pipe / Ctrl-D), NOT an error —
    // without this check a `loop`-on-invalid prompt would spin forever.
    if std::io::stdin().read_line(&mut s).context("read stdin")? == 0 {
        return Err(anyhow::anyhow!("unexpected end of input"));
    }
    Ok(s)
}

/// Restores terminal echo on drop — covers the normal return, the `?` error
/// path, AND a panic (Drop runs during unwind). Ctrl-C is handled separately in
/// [`prompt_hidden`] (a signal terminates without unwinding, so Drop alone
/// wouldn't run).
struct EchoGuard(bool);
impl Drop for EchoGuard {
    fn drop(&mut self) {
        if self.0 {
            let _ = std::process::Command::new("stty").arg("echo").status();
        }
    }
}

/// Read one line with terminal echo disabled (Unix, via `stty`). On a
/// non-terminal stdin (piped) or where `stty` is unavailable the input is read
/// visibly. Echo is restored on every exit path including Ctrl-C: the blocking
/// read runs on a worker thread raced against `ctrl_c`, and an `EchoGuard`
/// covers error/panic.
async fn prompt_hidden() -> Result<String> {
    use std::io::IsTerminal as _;
    // Only toggle echo for a real TTY; on a pipe we read the key as-is (and
    // `stty` would just error to stderr).
    let echo_off = std::io::stdin().is_terminal()
        && std::process::Command::new("stty")
            .arg("-echo")
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    let _guard = EchoGuard(echo_off);

    let read = tokio::task::spawn_blocking(|| {
        let mut s = String::new();
        std::io::stdin().read_line(&mut s).map(|n| (n, s))
    });
    tokio::select! {
        joined = read => {
            let (n, s) = joined.context("join stdin read")?.context("read stdin")?;
            if echo_off {
                eprintln!(); // the user's Enter wasn't echoed — emit the newline.
            }
            if n == 0 {
                return Err(anyhow::anyhow!("unexpected end of input"));
            }
            Ok(s)
        }
        _ = tokio::signal::ctrl_c() => {
            // `_guard` restores echo as it drops on the way out.
            Err(anyhow::anyhow!("cancelled"))
        }
    }
}

/// `xiaoguai init` — interactive setup wizard. Picks a provider from the local
/// registry, takes its API key (hidden), optionally makes it the default model,
/// and persists via the (already-tested) `provider::update`.
async fn handle_init(config: Option<&str>) -> Result<()> {
    use std::io::Write as _;
    let repo = build_provider_repo(config).await?;
    let providers = repo.list().await?;
    if providers.is_empty() {
        return Err(anyhow::anyhow!(
            "no providers found — run `xiaoguai serve` once so the migrations seed the defaults"
        ));
    }

    println!("xiaoguai setup — configure a model provider\n");
    print!("{}", init::format_provider_menu(&providers));

    let idx = loop {
        eprint!("\nPick a provider to configure [1-{}]: ", providers.len());
        std::io::stderr().flush().ok();
        if let Some(i) = init::parse_selection(&prompt_line()?, providers.len()) {
            break i;
        }
        eprintln!("  please enter a number between 1 and {}", providers.len());
    };
    let chosen = &providers[idx];

    eprint!(
        "\n{} API key (hidden — leave blank to keep the current one): ",
        chosen.name
    );
    std::io::stderr().flush().ok();
    let key_raw = prompt_hidden().await?;
    let key = {
        let k = key_raw.trim();
        if k.is_empty() {
            None
        } else {
            Some(k.to_string())
        }
    };

    eprint!(
        "\nMake {} the default model (so you can skip --model)? [Y/n]: ",
        chosen.name
    );
    std::io::stderr().flush().ok();
    let make_default = init::parse_yes_no(&prompt_line()?, true);

    // Warn if promoting a keyless cloud provider to default — it'll 401 on the
    // first request. Ollama needs no key, so don't nag there.
    let has_key = key.is_some() || chosen.api_key.is_some() || chosen.api_key_env.is_some();
    if make_default && !has_key && chosen.kind.as_str() != "ollama" {
        eprintln!(
            "  ! {} has no API key — as the default it will fail to authenticate. \
             Re-run init with a key, or: xiaoguai provider update --id {} --api-key-stdin",
            chosen.name,
            chosen.id.as_str()
        );
    }

    let repo_ref: &dyn LlmProviderRepository = &repo;
    let updated = provider::update(
        repo_ref,
        provider::UpdateArgs {
            id: chosen.id.as_str().to_string(),
            // fallback_order=0 makes this provider primary, so the router uses
            // its first model as the deployment default (see #214).
            fallback_order: if make_default { Some(0) } else { None },
            api_key: key.clone(),
            ..Default::default()
        },
    )
    .await?;

    eprintln!();
    if key.is_some() {
        eprintln!("✓ stored API key for {}", updated.name);
    }
    if make_default {
        let model = updated.models.first().map_or("(its model)", String::as_str);
        eprintln!(
            "✓ {} is now the default provider (default model: {model})",
            updated.name
        );
    }
    eprintln!("\nRestart the server for changes to take effect:  xiaoguai serve");
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
            api_key_stdin,
        } => {
            let api_key = if api_key_stdin {
                Some(read_api_key_from_stdin()?)
            } else {
                None
            };
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
                    api_key,
                },
            )
            .await?;
            println!("registered {} ({})", p.id, p.name);
        }
        ProviderCmd::Update {
            id,
            endpoint,
            models,
            default_for,
            fallback_order,
            api_key_env,
            api_key_stdin,
        } => {
            let api_key = if api_key_stdin {
                Some(read_api_key_from_stdin()?)
            } else {
                None
            };
            let p = provider::update(
                repo,
                provider::UpdateArgs {
                    id,
                    endpoint,
                    models: models.as_deref().map(split_csv),
                    default_for: default_for.as_deref().map(split_csv),
                    fallback_order,
                    api_key_env,
                    api_key,
                },
            )
            .await?;
            println!("updated {} ({})", p.id, p.name);
        }
        ProviderCmd::List => {
            let rows = provider::list(repo, provider::ListArgs {}).await?;
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
            auth,
            auth_url,
            token_url,
            client_id,
            scopes,
        } => {
            let args: Vec<String> = args.into_iter().filter(|s| !s.is_empty()).collect();
            let env_keys: Vec<String> = env_keys.into_iter().filter(|s| !s.is_empty()).collect();
            let scopes: Vec<String> = scopes.into_iter().filter(|s| !s.is_empty()).collect();
            match auth.as_deref() {
                None | Some("none") => {
                    let server = mcp::register(
                        repo,
                        mcp::RegisterArgs {
                            name,
                            version,
                            transport,
                            command,
                            args,
                            env_keys,
                            endpoint,
                        },
                    )
                    .await?;
                    println!(
                        "registered {} ({}@{})",
                        server.id, server.name, server.version
                    );
                }
                Some("oauth2-pkce") => {
                    use std::sync::Arc;
                    use xiaoguai_mcp::auth::{InMemoryTokenStore, TokenStore};
                    let (listener, redirect_uri) = mcp::bind_callback_listener().await?;
                    let base = mcp::RegisterArgs {
                        name,
                        version,
                        transport,
                        command,
                        args,
                        env_keys,
                        endpoint,
                    };
                    let oauth_args = mcp::OAuthRegisterArgs {
                        auth_url: auth_url.unwrap_or_default(),
                        token_url: token_url.unwrap_or_default(),
                        client_id: client_id.unwrap_or_default(),
                        scopes,
                    };
                    // In-memory store for the consent flow; production
                    // wiring of a SqliteTokenStore is a follow-up (see
                    // docs/plans/2026-05-29-tier3-oauth-pkce-outbound-mcp.md §7).
                    let store: Arc<dyn TokenStore> = Arc::new(InMemoryTokenStore::new());
                    let (server, bundle, _oauth_cfg) = mcp::register_oauth_with_listener(
                        repo,
                        store,
                        listener,
                        redirect_uri,
                        base,
                        oauth_args,
                    )
                    .await?;
                    println!(
                        "registered {} ({}@{})",
                        server.id, server.name, server.version
                    );
                    println!("oauth: access_token expires {}", bundle.expires_at);
                }
                Some(other) => {
                    return Err(anyhow::anyhow!(
                        "unknown --auth value {other:?}: expected 'oauth2-pkce' or 'none'"
                    ));
                }
            }
        }
        McpCmd::List => {
            let rows = mcp::list(repo, mcp::ListArgs {}).await?;
            print!("{}", mcp::format_table(&rows));
        }
        McpCmd::Remove { id } => {
            mcp::remove(repo, mcp::RemoveArgs { id: id.clone() }).await?;
            println!("removed {id}");
        }
    }
    Ok(())
}

/// Render one streamed `RemoteEvent` to the terminal: assistant text to stdout
/// (so it pipes cleanly), tool/done/error markers to stderr. Shared by
/// `remote chat` and `repl`.
fn render_remote_event(ev: &remote::RemoteEvent) {
    use std::io::Write as _;
    match ev.name.as_str() {
        "text_delta" => {
            if let Some(delta) = ev.payload.get("delta").and_then(serde_json::Value::as_str) {
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
}

/// Interactive multi-turn REPL against a running server. Creates one session
/// and loops: read a line from stdin → stream the reply. `/exit`, `/quit`, or
/// EOF (Ctrl-D) quits. The prompt marker goes to stderr so the assistant's
/// stdout text stays pipe-clean.
async fn handle_repl(server: String, user_id: String, model: String) -> Result<()> {
    use std::io::Write as _;
    let client = remote::RemoteClient::new(server.clone());
    client.healthz().await.with_context(|| {
        format!("could not reach the server at {server} — start it with `xiaoguai serve`")
    })?;
    let session = client
        .create_session(&remote::CreateSessionRequest {
            user_id,
            model,
            title: None,
        })
        .await?;
    eprintln!(
        "xiaoguai repl — session {} (type /exit or Ctrl-D to quit)",
        session.id
    );

    let stdin = std::io::stdin();
    loop {
        eprint!("\n> ");
        std::io::stderr().flush().ok();
        let mut line = String::new();
        if stdin.read_line(&mut line).context("read stdin")? == 0 {
            eprintln!();
            break; // EOF / Ctrl-D
        }
        let prompt = line.trim();
        if prompt.is_empty() {
            continue;
        }
        if matches!(prompt, "/exit" | "/quit") {
            break;
        }
        if let Err(e) = client
            .send_message(&session.id, prompt, |ev| {
                render_remote_event(&ev);
                Ok(())
            })
            .await
        {
            // Keep the REPL alive on a per-turn error (network blip, etc.).
            eprintln!("[error] {e:#}");
        }
    }
    eprintln!("bye");
    Ok(())
}

async fn handle_remote(server: String, action: RemoteCmd) -> Result<()> {
    let client = remote::RemoteClient::new(server);
    match action {
        RemoteCmd::Healthz => {
            let body = client.healthz().await?;
            println!("{body}");
        }
        RemoteCmd::Chat {
            user_id,
            model,
            prompt,
            title,
        } => {
            let session = client
                .create_session(&remote::CreateSessionRequest {
                    user_id,
                    model,
                    title,
                })
                .await?;
            eprintln!("session: {}", session.id);
            client
                .send_message(&session.id, &prompt, |ev| {
                    render_remote_event(&ev);
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
                scope,
                window_secs,
                max_count,
                max_usd,
                escalate_to,
            } => {
                let v = hotl::policy_create(hotl::PolicyCreateArgs {
                    api_base,
                    scope,
                    window_secs,
                    max_count,
                    max_usd,
                    escalate_to,
                })
                .await?;
                print_value(&v, &output)?;
            }
            HotlPolicyCmd::List { scope } => {
                let rows = hotl::policy_list(hotl::PolicyListArgs { api_base, scope }).await?;
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
        HotlCmd::Check { scope, amount } => {
            let resp = hotl::check(hotl::CheckArgs {
                api_base,
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
            agent_name,
            kind,
            value,
            session_id,
            unit,
            description,
        } => {
            let v = outcomes::record(outcomes::RecordArgs {
                api_base,
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
        OutcomesCmd::List { range, kind, limit } => {
            let rows = outcomes::list(outcomes::ListArgs {
                api_base,
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
        OutcomesCmd::Summary { range } => {
            let rows = outcomes::summary(outcomes::SummaryArgs { api_base, range }).await?;
            if output == "table" {
                print!("{}", outcomes::format_summary_table(&rows));
            } else {
                print_value(&serde_json::to_value(&rows)?, &output)?;
            }
        }
        OutcomesCmd::Timeseries { range, kind } => {
            let v = outcomes::timeseries(outcomes::TimeseriesArgs {
                api_base,
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
            category,
            installed,
        } => {
            let rows = skills::list(skills::ListArgs {
                api_base,
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
        SkillsCmd::Install { pack, config } => {
            let v = skills::install(skills::InstallArgs {
                api_base,
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
        SkillsCmd::Proposals { action } => {
            handle_proposals(api_base, output, action).await?;
        }
    }
    Ok(())
}

async fn handle_proposals(api_base: String, output: String, action: ProposalsCmd) -> Result<()> {
    match action {
        ProposalsCmd::List { status } => {
            let rows = skills::proposals_list(&api_base, status.as_deref()).await?;
            if output == "table" {
                print!("{}", skills::format_proposals_table(&rows));
            } else {
                print_value(&serde_json::to_value(&rows)?, &output)?;
            }
        }
        ProposalsCmd::Approve { id, decided_by } => {
            let v = skills::proposals_approve(&api_base, &id, &decided_by).await?;
            print_value(&v, &output)?;
        }
        ProposalsCmd::Reject {
            id,
            decided_by,
            reason,
        } => {
            let v = skills::proposals_reject(&api_base, &id, &decided_by, &reason).await?;
            print_value(&v, &output)?;
        }
    }
    Ok(())
}

async fn handle_watch(api_base: String, output: String, action: WatchCmd) -> Result<()> {
    match action {
        WatchCmd::List => {
            let rows = watch::list(watch::ListArgs { api_base }).await?;
            if output == "table" {
                print!("{}", watch::format_list_table(&rows));
            } else {
                print_value(&serde_json::to_value(&rows)?, &output)?;
            }
        }
        WatchCmd::Start { file } => {
            let v = watch::start(watch::StartArgs {
                api_base,
                file: std::path::PathBuf::from(file),
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
    let cli = Cli::parse();
    // ACP owns stdout for its JSON-RPC stream, so its logs MUST go to stderr;
    // every other subcommand keeps the default stdout logger.
    if matches!(cli.command, Cmd::Acp) {
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .init();
    } else {
        tracing_subscriber::fmt::init();
    }
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
        Cmd::Acp => {
            let settings = xiaoguai_core::load_settings(cfg.map(std::path::Path::new))
                .context("load settings for acp")?;
            xiaoguai_core::acp_bridge::run_acp(&settings).await
        }
        Cmd::Chat {
            prompt,
            mock,
            ollama_url,
            model,
        } => handle_chat(prompt, mock, ollama_url, model).await,
        Cmd::Provider { action } => handle_provider(cfg, action).await,
        Cmd::Init => handle_init(cfg).await,
        Cmd::Mcp { action } => handle_mcp(cfg, action).await,
        Cmd::Remote { server, action } => handle_remote(server, action).await,
        Cmd::Repl {
            server,
            user_id,
            model,
        } => handle_repl(server, user_id, model).await,
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
            restore_db,
        } => {
            let restore_db_to = restore_db.map(|url| backup::resolve_sqlite_path(&url));
            backup::run_restore(backup::RestoreArgs {
                input: std::path::PathBuf::from(input),
                outdir: std::path::PathBuf::from(outdir),
                force,
                identity: identity.map(std::path::PathBuf::from),
                restore_db_to,
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
        Cmd::Stats {
            by,
            since,
            until,
            json,
        } => handle_stats(cfg, by, since, until, json).await,
        Cmd::Code { workspace, action } => {
            let settings = load_settings(cfg)?;
            let ws = std::path::Path::new(&workspace);
            match action {
                CodeCmd::Status => code::status(&settings, ws).await,
                CodeCmd::Write { path, content } => {
                    code::write(&settings, ws, std::path::Path::new(&path), content).await
                }
                CodeCmd::Commit { message } => code::commit(&settings, ws, message).await,
                CodeCmd::Rollback { checkpoint } => code::rollback(&settings, ws, checkpoint).await,
                CodeCmd::Push { branch, remote } => code::push(&settings, ws, remote, branch).await,
                CodeCmd::OpenPr { title, body, base } => {
                    code::open_pr(&settings, ws, title, body, base).await
                }
            }
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
            framework,
            from,
            to,
            output,
            format,
        } => {
            audit_export::run(audit_export::ExportArgs {
                api_base,
                framework,
                from,
                to,
                output: std::path::PathBuf::from(output),
                format,
            })
            .await
        }
        AuditCmd::Bundle {
            framework,
            from,
            to,
            out,
        } => {
            audit_bundle::run(audit_bundle::BundleArgs {
                api_base,
                framework,
                from,
                to,
                out_dir: std::path::PathBuf::from(out),
            })
            .await
        }
    }
}
