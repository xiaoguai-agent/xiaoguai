//! clap CLI argument definitions, extracted from main.rs (Phase E / DEC-041).
//!
//! Pure `clap` derive types (`Cli` + the subcommand enums). Dispatch and the
//! command bodies stay in `main.rs`; this module is just the parse surface.

use clap::{Parser, Subcommand};

// `Completions { shell }` carries the shell enum re-exported by the completions
// command module.
use xiaoguai_cli::commands::completions;

#[derive(Parser)]
#[command(name = "xiaoguai", version, about = "Xiaoguai CLI")]
pub struct Cli {
    /// Path to a YAML config file. Defaults to `~/.xiaoguai/config.yaml` if
    /// the file exists, otherwise an env-driven default.
    #[arg(long, global = true)]
    pub config: Option<String>,

    #[command(subcommand)]
    pub command: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Run the long-lived API server (was the `xiaoguai-core` binary).
    ///
    /// Reads the same YAML config as `xiaoguai-core` and the same
    /// `DATABASE_URL` / `XIAOGUAI_AUDIT_SIGNING_KEY` / `OLLAMA_HOST`
    /// environment. The legacy `xiaoguai-core` binary is now a thin shim
    /// over the same library entry point.
    Serve {
        /// Bind address (overrides config/env). Default `127.0.0.1` (local
        /// only). Use `--host 0.0.0.0` to reach it from your LAN — that needs
        /// owner auth (set `XIAOGUAI_AUTH__USERNAME` + `XIAOGUAI_AUTH__PASSWORD`)
        /// per SEC-01, or the explicit `XIAOGUAI_ALLOW_UNAUTHENTICATED_NONLOOPBACK=1`.
        /// Find your LAN IP with `hostname -I`.
        #[arg(long)]
        host: Option<String>,
        /// Port to bind (overrides config/env). Default `7600`.
        #[arg(long)]
        port: Option<u16>,
    },

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

    /// Start an interactive chat session against a running server. Multi-turn
    /// (keeps history) and uses your registered providers (`MiniMax`, etc.).
    /// Reads prompts from stdin; `/help` for commands, `/exit` or Ctrl-D quits.
    ///
    /// Spelled `xiaoguai cli` (also `xiaoguai start`); `xiaoguai repl` still
    /// works as a back-compat alias.
    ///
    /// If the server has the owner auth gate enabled, set
    /// `XIAOGUAI_AUTH__USERNAME` + `XIAOGUAI_AUTH__PASSWORD` in this shell (the
    /// same values `serve` uses) — otherwise the server replies `401`.
    #[command(name = "cli", visible_alias = "start", alias = "repl")]
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

    /// Interactive setup wizard: pick a provider, enter its API key, and
    /// optionally make it the default model. Writes to the local DB; no web
    /// UI needed. Restart `xiaoguai serve` afterwards.
    Init {
        /// Show the API key as you type it instead of hiding it. Off by
        /// default (the key is hidden like a password prompt); either way a
        /// masked confirmation is echoed so you can verify what was captured.
        #[arg(long)]
        plaintext: bool,
    },

    /// Self-check the local install: database writable, providers + default
    /// key, Ollama reachability/model (when an Ollama provider is default),
    /// and whether the serve port is free or already serving.
    ///
    /// Prints a ✓/!/✗ table. Exits 1 only on hard ✗ failures — warnings
    /// (e.g. the default Ollama model not pulled yet) keep exit code 0.
    Doctor,

    /// Install, remove, or inspect the background service that keeps
    /// `xiaoguai serve` running (systemd on Linux — needs sudo; a per-user
    /// launchd agent on macOS — no root). Windows is not supported (use
    /// Docker or WSL).
    Service {
        #[command(subcommand)]
        action: ServiceCmd,
    },

    /// Send a one-shot prompt and stream the reply.
    ///
    /// By default this talks to a running `xiaoguai serve` over HTTP: it
    /// auto-creates a session, sends the prompt, and streams the reply using
    /// your registered providers (`MiniMax`, etc.), `HotL` gating, and audit —
    /// no manual session id to juggle. Pass `--mock` or `--ollama-url` to
    /// bypass the server and hit a backend directly (offline / dev).
    Chat {
        /// User prompt.
        #[arg(long)]
        prompt: String,
        /// Base URL of the API server (server mode; ignored with
        /// `--mock` / `--ollama-url`).
        #[arg(long, default_value = "http://localhost:7600")]
        server: String,
        /// User id for the auto-created session (server mode).
        #[arg(long, default_value = "usr_dev")]
        user_id: String,
        /// Bypass the server: use the deterministic mock backend (no network).
        #[arg(long, conflicts_with = "ollama_url")]
        mock: bool,
        /// Bypass the server: hit Ollama directly at this base URL
        /// (default <http://localhost:11434>).
        #[arg(long)]
        ollama_url: Option<String>,
        /// Model name. Empty (the default) lets the server pick its default
        /// model in server mode, or falls back to `qwen2.5-coder` for Ollama.
        #[arg(long, default_value = "")]
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

    /// Manage scheduled jobs (cron-triggered agent prompts, `SQLite`-backed).
    ///
    /// Jobs are executed by the scheduler inside `xiaoguai serve`; the CLI
    /// writes the same tables the runner polls, so changes apply without a
    /// restart. `run-now` talks to the running server over HTTP.
    Schedule {
        #[command(subcommand)]
        action: ScheduleCmd,
    },

    /// Talk to a running `xiaoguai-api` over HTTP/SSE.
    Remote {
        /// Base URL of the API server, e.g. `http://localhost:7600`.
        #[arg(long, default_value = "http://localhost:7600")]
        server: String,
        #[command(subcommand)]
        action: RemoteCmd,
    },

    /// Manage session-scoped recurring agent loops (DEC-039 /loop).
    ///
    /// A loop re-issues a prompt to a session on a fixed interval, each
    /// tick building on what previous ticks learned, until a budget trips
    /// or you cancel it. Use this for "keep watching what we were just
    /// talking about"; use `schedule` for fixed-calendar cron jobs. The
    /// loop runs inside `xiaoguai serve`, so these commands go over HTTP.
    Loop {
        /// Base URL of the API server, e.g. `http://localhost:7600`.
        #[arg(long, default_value = "http://localhost:7600")]
        server: String,
        #[command(subcommand)]
        action: LoopCmd,
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

    /// Bulk-transfer long-term memories as JSONL against the local store
    /// (T7.2). Runs directly over `SQLite` — no server needed; imports
    /// re-embed with the configured embedder (`memory.embedder` config,
    /// `OLLAMA_HOST` override), same as `xiaoguai serve`.
    Memory {
        #[command(subcommand)]
        action: MemoryCmd,
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

    /// Validate skill-pack manifests (offline; parse + path checks, no side effects).
    Pack {
        #[command(subcommand)]
        action: PackCmd,
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
pub enum AuditCmd {
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
pub enum HotlCmd {
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
    /// List pending decisions awaiting an operator — including /loop ticks
    /// parked on a gated tool. Resolve one with `POST /v1/hotl/decisions`.
    Pending,
}

#[derive(Subcommand)]
pub enum HotlPolicyCmd {
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
pub enum OutcomesCmd {
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
pub enum SkillsCmd {
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
pub enum ProposalsCmd {
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
pub enum WatchCmd {
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
pub enum AnomalyCmd {
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

/// `xiaoguai pack ...` — skill-pack manifest tooling (Phase 1: validate only).
#[derive(Subcommand)]
pub enum PackCmd {
    /// Parse + validate a pack manifest (offline, no side effects).
    ///
    /// Loads `pack.yaml`, confirms every declared migration/watch/anomaly/agent
    /// path exists, and reports what would register. Exits non-zero if the pack
    /// would fail to load.
    Validate {
        /// Path to the pack directory (containing `pack.yaml`) or the file itself.
        dir: String,
    },
    /// Install a pack: validate it, then record it (enabled) in the embedded
    /// store so the next `serve` boot wires its anomaly specs as scheduled jobs.
    Install {
        /// Path to the pack directory (containing `pack.yaml`).
        dir: String,
    },
}

// ---------------------------------------------------------------------------
// Existing sub-enums (unchanged)
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum EvalCmd {
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
pub enum RemoteCmd {
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
    /// Run a complex goal through an expert team — members work in parallel,
    /// the lead synthesizes one answer — against a fresh session, streaming
    /// progress and the final result. The team-based version of `chat`, for
    /// tasks worth several perspectives.
    Orchestrate {
        #[arg(long)]
        user_id: String,
        /// The goal/task to hand the team.
        #[arg(long)]
        goal: String,
        /// Team id to use. Omit to auto-route the goal to the best-matching
        /// active team (422 if none matches — create one in the admin console
        /// or chat-ui Expert picker first).
        #[arg(long)]
        team: Option<String>,
        /// Cap how many members run in parallel (the server clamps to 1–8).
        #[arg(long)]
        max_members: Option<usize>,
    },
}

#[derive(Subcommand)]
pub enum LoopCmd {
    /// Create + arm a loop on a session.
    Create {
        /// Session the loop's ticks run in.
        #[arg(long)]
        session: String,
        /// Prompt re-issued to the session each tick.
        #[arg(long)]
        prompt: String,
        /// Seconds between ticks (default 300).
        #[arg(long)]
        interval_secs: Option<u32>,
        /// Stop after this many ticks (default 50).
        #[arg(long)]
        max_ticks: Option<u32>,
        /// Stop after this many seconds of wall-clock life (default 86400).
        #[arg(long)]
        ttl_secs: Option<u32>,
        /// Let the agent pace the loop via `loop_next_tick` (dynamic pacing).
        #[arg(long)]
        dynamic_pacing: bool,
        /// Dynamic-pacing lower bound, seconds (default 10).
        #[arg(long)]
        min_interval_secs: Option<u32>,
        /// Dynamic-pacing upper bound, seconds (default 3600).
        #[arg(long)]
        max_interval_secs: Option<u32>,
        /// Stop once the session burns this many tokens (default 500000;
        /// 0 = unlimited).
        #[arg(long)]
        max_total_tokens: Option<u64>,
    },
    /// List all loops (active and terminal), newest first.
    List,
    /// Show one loop by id (or unique id prefix).
    Show {
        #[arg(long)]
        id: String,
    },
    /// Cancel a live loop by id (or unique id prefix).
    Cancel {
        #[arg(long)]
        id: String,
    },
    /// Resume a paused loop by id (or unique id prefix).
    Resume {
        #[arg(long)]
        id: String,
    },
}

#[derive(Subcommand)]
// Tier-3 T4 added enough OAuth flags to the `Register` variant that
// `clippy::large_enum_variant` fires. Boxing the strings would make
// the clap derive output noisier than the warning warrants — clap
// allocates these once at parse time and they live for the program
// lifetime.
#[allow(clippy::large_enum_variant)]
pub enum McpCmd {
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
pub enum ProviderCmd {
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
pub enum ScheduleCmd {
    /// Create a cron-scheduled job that runs an agent prompt.
    Create {
        /// Human-readable job name.
        #[arg(long)]
        name: String,
        /// 6-field cron expression (sec min hour day-of-month month
        /// day-of-week), evaluated in UTC — e.g. '0 0 8 * * *' = daily 08:00.
        #[arg(long)]
        cron: String,
        /// Agent prompt the job runs on each fire.
        #[arg(long)]
        prompt: String,
        /// Optional free-form description.
        #[arg(long)]
        description: Option<String>,
        /// Push sink for results, e.g. `feishu:chat-x`, `inbox:owner`.
        /// Repeatable. Omit to keep results in the run history only.
        #[arg(long = "sink")]
        sinks: Vec<String>,
    },
    /// List scheduled jobs (active and paused).
    List {
        /// Maximum rows to show.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Show one job in full, including recent run history. Accepts the
    /// short id from `list` (any unique prefix) or the full id.
    Show { id: String },
    /// Pause a job (it stays in the table; the runner skips it).
    Pause { id: String },
    /// Resume a paused job; the next fire is recomputed from now.
    Resume { id: String },
    /// Delete a job and its run history (asks for confirmation).
    Delete {
        id: String,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Fire a job immediately via the running server.
    RunNow {
        id: String,
        /// Base URL of the `xiaoguai-api` server.
        #[arg(
            long,
            env = "XIAOGUAI_API_BASE",
            default_value = "http://localhost:7600"
        )]
        server: String,
    },
}

#[derive(Subcommand)]
pub enum TasksCmd {
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
pub enum CodeCmd {
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

#[derive(Subcommand)]
pub enum MemoryCmd {
    /// Export memories as JSONL (one `{kind, content, tags, ttl_at,
    /// created_at}` object per line; embeddings are not exported).
    Export {
        /// Only export this kind: 'facts', 'episodes' or 'preferences'.
        #[arg(long)]
        kind: Option<String>,
        /// Write to this file instead of stdout.
        #[arg(long)]
        out: Option<String>,
    },
    /// Import a JSONL file. Fail-soft: blank lines are skipped silently,
    /// malformed lines are reported with their line number; valid lines are
    /// re-embedded and tagged `source:imported` unless they already carry a
    /// `source:` tag.
    Import {
        /// Path to the JSONL file.
        file: String,
    },
}

#[derive(Subcommand)]
pub enum ServiceCmd {
    /// Install and start the background service (systemd unit on Linux —
    /// requires sudo; per-user launchd agent on macOS — no root needed).
    /// Idempotent: re-running refreshes the unit/plist and restarts it.
    Install {
        /// Render the unit/plist and print the target paths without
        /// touching the system (no writes, no systemctl/launchctl).
        #[arg(long)]
        print_only: bool,
    },
    /// Stop and remove the background service. Data (the `SQLite` store, the
    /// service user on Linux) is left in place.
    Uninstall,
    /// Show the service status (systemctl status / launchctl list).
    Status,
}
