//! Configuration loader for Xiaoguai.
//!
//! Layering (highest precedence first):
//!   1. Environment variables (`XIAOGUAI_*` prefix, double-underscore separator
//!      maps to nested keys — e.g. `XIAOGUAI_DATABASE__URL` overrides `database.url`)
//!   2. `config.yaml` (path passed to `Settings::load_from_file`)
//!   3. Compiled-in defaults
//!
//! CLI flags layer on top via the binary's own `clap` parser.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub server: ServerSettings,
    pub database: DatabaseSettings,
    pub cache: CacheSettings,
    pub auth: AuthSettings,
    pub audit: AuditSettings,
    /// Scheduler-side configuration. Optional so existing
    /// config.yaml files from v0.10.0/v0.10.1 still deserialize.
    /// v0.10.3 carries push-sink config under `scheduler.sinks.*`.
    #[serde(default)]
    pub scheduler: SchedulerSettings,
    /// v0.7.4: IM gateway runtime knobs.
    #[serde(default)]
    pub im: ImSettings,
    /// v0.11.2: eval pane substrate. Optional so existing
    /// config.yaml files still deserialize; defaults to disabled
    /// (`suites_dir = "./eval-suites"`, endpoints return 503 when
    /// the directory doesn't exist).
    #[serde(default)]
    pub eval: EvalSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSettings {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseSettings {
    pub url: String,
    #[serde(default = "default_pg_max_connections")]
    pub max_connections: u32,
}

const fn default_pg_max_connections() -> u32 {
    16
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheSettings {
    pub url: String,
    #[serde(default = "default_cache_prefix")]
    pub key_prefix: String,
}

fn default_cache_prefix() -> String {
    "xiaoguai:".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSettings {
    /// Expected JWT `iss` value.
    pub issuer: String,
    /// Expected JWT `aud` value.
    pub audience: String,
    /// JWKS URL (e.g. `https://idp.example.com/.well-known/jwks.json`).
    pub jwks_url: String,
    /// When `true`, the API server requires a Bearer JWT on `/v1/**` and
    /// the rbac middleware enforces Casbin policies. When `false`
    /// (default) the server runs in dev mode: claims fall back to the
    /// request body, and rbac is bypassed.
    ///
    /// Override via `XIAOGUAI_AUTH__REQUIRED=true`.
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditSettings {
    /// HMAC-SHA256 signing key for the audit chain. **NEVER** check in a real key.
    /// In production load via env or external secrets manager.
    pub hmac_key: String,
    /// v0.6.5: env-var name to read the production audit signing key from
    /// when wiring `PgAuditSink` in `xiaoguai-core`. The dev `hmac_key`
    /// above is fine for `smoke` and tests but must NOT be used for the
    /// production audit chain — operators set this knob and stash the
    /// real key in the named env var.
    #[serde(default = "default_signing_key_env")]
    pub signing_key_env: String,
}

fn default_signing_key_env() -> String {
    "XIAOGUAI_AUDIT_SIGNING_KEY".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerSettings {
    /// v0.12.0: when `true`, `xiaoguai-core` spawns the `JobRunner`
    /// on a tokio task. Off by default so existing deployments don't
    /// change behaviour. Override via `XIAOGUAI_SCHEDULER__ENABLED=true`.
    #[serde(default)]
    pub enabled: bool,
    /// v0.12.0: how often the runner walks `scheduled_jobs` for due
    /// rows (the `JobRunner::run_loop` timer arm). Reactive triggers
    /// fire via the event channel regardless of this knob.
    #[serde(default = "default_tick_interval_secs")]
    pub tick_interval_secs: u64,
    /// Per-sink config blocks. Every field is `Option<_>` so an
    /// operator wires only the sinks they actually deploy.
    #[serde(default)]
    pub sinks: SchedulerSinkSettings,
    /// v0.12.2: filesystem-watch source bootstrap. Off by default so
    /// existing deployments keep the v0.12.0 webhook-only behaviour.
    /// Routes come from two places: the static `routes` list here
    /// (config-defined, ops-friendly) AND every persisted
    /// `scheduled_jobs` row whose `trigger.type == "file_watch"`
    /// (operator-friendly, edit a job in the admin pane to add a
    /// watch). Both lists merge into one [`FileWatchSource`].
    #[serde(default)]
    pub file_watch: FileWatchSettings,
}

impl Default for SchedulerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            tick_interval_secs: default_tick_interval_secs(),
            sinks: SchedulerSinkSettings::default(),
            file_watch: FileWatchSettings::default(),
        }
    }
}

/// v0.12.2: file-watch source configuration block.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileWatchSettings {
    /// When `true`, `xiaoguai-core` instantiates a `FileWatchSource`,
    /// registers every [`FileWatchRoute`] below, optionally scans the
    /// `scheduled_jobs` table for additional `file_watch` triggers,
    /// then starts the source against the existing scheduler event
    /// channel. Off by default — operators flip
    /// `XIAOGUAI_SCHEDULER__FILE_WATCH__ENABLED=true` to opt in.
    #[serde(default)]
    pub enabled: bool,
    /// Static routes. One entry per `(job_id, path)` binding. Empty by
    /// default; operators add entries in `config.yaml` for ops
    /// scenarios that shouldn't require a DB write.
    #[serde(default)]
    pub routes: Vec<FileWatchRoute>,
    /// When `true` AND `enabled` is true, `xiaoguai-core` scans
    /// `scheduled_jobs` for rows whose trigger type is `file_watch`
    /// and registers each as a route automatically. Defaults to `true`
    /// because the DB-driven path is the operator-friendly default;
    /// disable when you want the static `routes` list to be the
    /// exclusive source.
    #[serde(default = "default_load_routes_from_db")]
    pub load_routes_from_db: bool,
}

const fn default_load_routes_from_db() -> bool {
    true
}

/// One static `(job_id, path)` binding for the file-watch source.
///
/// Mirrors `xiaoguai_scheduler::FileWatchRoute` in shape — kept here
/// (not imported) so the config crate stays independent of the
/// scheduler crate. The operator binary converts these into the
/// scheduler type at boot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileWatchRoute {
    pub job_id: String,
    pub path: String,
}

const fn default_tick_interval_secs() -> u64 {
    30
}

/// Container for the four real `PushSink` configs shipped in
/// v0.10.3. Fields stay as opaque `serde_json::Value` so the
/// scheduler crate (which owns the strongly-typed
/// `FeishuSinkConfig` / `TelegramSinkConfig` / etc.) can deserialize
/// them lazily without forcing this crate to depend on
/// `xiaoguai-scheduler`. The operator binary calls
/// `serde_json::from_value::<FeishuSinkConfig>(cfg.scheduler.sinks
/// .feishu.clone().unwrap())` when constructing the sink.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchedulerSinkSettings {
    #[serde(default)]
    pub feishu: Option<serde_json::Value>,
    #[serde(default)]
    pub telegram: Option<serde_json::Value>,
    #[serde(default)]
    pub email: Option<serde_json::Value>,
    /// Inbox needs no config (in-process FIFO) but the toggle stays
    /// here so an operator can disable it without touching the
    /// binary's wiring code.
    #[serde(default)]
    pub inbox: Option<serde_json::Value>,
}

/// v0.7.4: IM gateway runtime knobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImSettings {
    /// When `true`, `xiaoguai-core` keeps the v0.7.2 in-process
    /// `ConversationHistory` even when a PG pool is available. Production
    /// HA deployments should leave this `false` so multi-replica
    /// webhooks stay consistent.
    ///
    /// Override via `XIAOGUAI_IM__USE_IN_PROCESS_HISTORY=true`.
    #[serde(default)]
    pub use_in_process_history: bool,

    /// Per-conversation replay-window cap. The IM history store reads at
    /// most this many trailing turns when assembling the agent's input —
    /// older messages stay in the DB (for audit) but are not replayed.
    /// Default 50.
    #[serde(default = "default_max_messages_per_conversation")]
    pub max_messages_per_conversation: usize,
}

impl Default for ImSettings {
    fn default() -> Self {
        Self {
            use_in_process_history: false,
            max_messages_per_conversation: default_max_messages_per_conversation(),
        }
    }
}

const fn default_max_messages_per_conversation() -> usize {
    50
}

/// v0.11.2: eval pane substrate. `suites_dir` points at a directory of
/// `*.eval.yaml` case files (or subdirectories holding them). When the
/// directory doesn't exist the eval endpoints stay disabled — same
/// trust model as `audit` (503 instead of silently making something up).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalSettings {
    #[serde(default = "default_eval_suites_dir")]
    pub suites_dir: String,
    /// Hard cap on agent-loop iterations per case. Zero = use the
    /// xiaoguai-agent default. Mirrors the `xiaoguai eval run
    /// --max-iterations` CLI knob.
    #[serde(default)]
    pub max_iterations: u32,
}

impl Default for EvalSettings {
    fn default() -> Self {
        Self {
            suites_dir: default_eval_suites_dir(),
            max_iterations: 0,
        }
    }
}

fn default_eval_suites_dir() -> String {
    "./eval-suites".into()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server: ServerSettings {
                host: "0.0.0.0".into(),
                port: 7600,
            },
            database: DatabaseSettings {
                url: "postgres://xiaoguai:xiaoguai@localhost:5432/xiaoguai".into(),
                max_connections: default_pg_max_connections(),
            },
            cache: CacheSettings {
                url: "redis://localhost:6379".into(),
                key_prefix: default_cache_prefix(),
            },
            auth: AuthSettings {
                issuer: "https://idp.example.com".into(),
                audience: "xiaoguai-core".into(),
                jwks_url: "https://idp.example.com/.well-known/jwks.json".into(),
                required: false,
            },
            audit: AuditSettings {
                hmac_key: "dev-only-change-me-32-bytes-min".into(),
                signing_key_env: default_signing_key_env(),
            },
            scheduler: SchedulerSettings::default(),
            im: ImSettings::default(),
            eval: EvalSettings::default(),
        }
    }
}

impl Settings {
    /// Load settings from a YAML file + environment overrides.
    ///
    /// Environment variables use the `XIAOGUAI_` prefix and `__` separator,
    /// e.g. `XIAOGUAI_DATABASE__URL=postgres://...`.
    ///
    /// # Errors
    /// Returns a textual error if the file cannot be read or parsed.
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self, String> {
        let cfg = ::config::Config::builder()
            .add_source(::config::File::from(path.as_ref()))
            .add_source(::config::Environment::with_prefix("XIAOGUAI").separator("__"))
            .build()
            .map_err(|e| e.to_string())?;
        cfg.try_deserialize().map_err(|e| e.to_string())
    }

    /// Load settings from environment overrides only (uses defaults as base).
    ///
    /// # Errors
    /// Returns a textual error if env vars fail to deserialize.
    pub fn load_from_env() -> Result<Self, String> {
        let defaults_yaml = serde_yaml::to_string(&Self::default()).map_err(|e| e.to_string())?;
        let cfg = ::config::Config::builder()
            .add_source(::config::File::from_str(
                &defaults_yaml,
                ::config::FileFormat::Yaml,
            ))
            .add_source(::config::Environment::with_prefix("XIAOGUAI").separator("__"))
            .build()
            .map_err(|e| e.to_string())?;
        cfg.try_deserialize().map_err(|e| e.to_string())
    }
}
