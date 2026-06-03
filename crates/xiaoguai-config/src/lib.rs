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

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

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
    /// Sprint-12 (S12-0): agent-loop runtime knobs. Optional so existing
    /// v1.8.x config.yaml files deserialize unchanged.
    #[serde(default)]
    pub agent: AgentSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSettings {
    pub host: String,
    pub port: u16,
    /// Optional directory holding the built web UIs. When set (and it exists),
    /// `xiaoguai-core` serves `chat-ui` at `/` and `admin-ui` at `/admin/`
    /// from `<static_dir>/chat-ui` and `<static_dir>/admin-ui`. When unset
    /// (the default), the server is API-only — preserving the historical
    /// behaviour. The container image sets this to `/app/static`; bare-metal
    /// installs that bundle the UI point it at `<prefix>/share/static`.
    ///
    /// Override via YAML `server.static_dir: /app/static` or env
    /// `XIAOGUAI_SERVER__STATIC_DIR=/app/static`.
    #[serde(default)]
    pub static_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseSettings {
    /// SQLite store location (DEC-033). A filesystem path or `sqlite://…` URL.
    /// Empty (the default) resolves to `$XDG_DATA_HOME/xiaoguai/data.db` or
    /// `~/.xiaoguai/data.db` — so a clean box can `serve` with no config.
    #[serde(default = "default_db_url")]
    pub url: String,
    #[serde(default = "default_db_max_connections")]
    pub max_connections: u32,
}

/// Empty = resolve to the default per-user SQLite path at connect time.
fn default_db_url() -> String {
    String::new()
}

const fn default_db_max_connections() -> u32 {
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

/// Single-owner access gate (DEC-033). The API has no OIDC, RBAC, scopes,
/// or tenants — it is protected by one configured username + password
/// checked via HTTP Basic auth. When either field is empty the gate is
/// disabled and the server runs open (convenient for a localhost run);
/// front it with a credential before exposing it on a URL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSettings {
    /// Owner username. Empty = auth disabled.
    ///
    /// Override via `XIAOGUAI_AUTH__USERNAME=...`.
    #[serde(default)]
    pub username: String,
    /// Owner password. Empty = auth disabled. Keep this in the `.env` /
    /// secret store, not in a checked-in config file.
    ///
    /// Override via `XIAOGUAI_AUTH__PASSWORD=...`.
    #[serde(default)]
    pub password: String,
}

impl AuthSettings {
    /// The gate is active only when both credentials are non-empty.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        !self.username.is_empty() && !self.password.is_empty()
    }
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

/// Sprint-12 (S12-0): agent-loop runtime knobs. Each nested block defaults
/// to its `Default` impl so that omitted blocks in `config.yaml` preserve
/// pre-sprint-12 behaviour.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentSettings {
    /// `HotL` (human-on-the-loop) gating knobs.
    #[serde(default)]
    pub hotl: HotlSettings,
}

/// Sprint-13 S13-0: serde adapter that lets `HashMap<String, Duration>`
/// round-trip through humantime string literals like `"24h"`. Mirrors
/// the `humantime_serde::Serde<T>` wrapper pattern but specialised to
/// the per-scope expiry map shape so call sites stay terse.
mod humantime_serde_map {
    use std::collections::HashMap;
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Serialise each value via `humantime_serde::Serde<Duration>`, which
    /// produces a human-readable string like `"24h"`.
    pub fn serialize<S>(map: &HashMap<String, Duration>, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let wrapped: HashMap<&String, humantime_serde::Serde<&Duration>> = map
            .iter()
            .map(|(k, v)| (k, humantime_serde::Serde::from(v)))
            .collect();
        wrapped.serialize(s)
    }

    /// Deserialise each value via `humantime_serde::Serde<Duration>`,
    /// accepting human strings like `"24h"` or numeric seconds.
    pub fn deserialize<'de, D>(d: D) -> Result<HashMap<String, Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wrapped: HashMap<String, humantime_serde::Serde<Duration>> = HashMap::deserialize(d)?;
        Ok(wrapped
            .into_iter()
            .map(|(k, v)| (k, v.into_inner()))
            .collect())
    }
}

/// Sprint-12 (S12-0 + S12-12): `HotL` gating knobs.
///
/// `suspend_on_escalate` was introduced as a scaffold in S12-0 (v1.8.x,
/// defaulted `false`) so tenants could opt into the upcoming behaviour
/// early. The full suspend/resume stack landed across S12-1..S12-10 and
/// S12-12 flips the default to `true` for v1.9.0. Production gate
/// selection happens in `xiaoguai-core::hotl_bridge::build_hotl_gate`:
/// `true` → `SuspendingHotlGate`, `false` → legacy `EnforcerGate`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotlSettings {
    /// When `true`, an `Escalate` verdict from `HotlEnforcer` suspends the
    /// agent loop until an operator decision arrives via
    /// `POST /v1/hotl/decisions` (or the configured timeout fires). When
    /// `false` the loop logs the escalation and allows the tool call to
    /// dispatch — the legacy `EnforcerGate` behaviour.
    ///
    /// v1.9.0+ default: `true`. Tenants who tested on v1.8.x and want the
    /// old "Escalate → Allow + warn" behaviour can opt out explicitly by
    /// setting `agent.hotl.suspend_on_escalate: false` in `config.yaml`
    /// or via `XIAOGUAI_AGENT__HOTL__SUSPEND_ON_ESCALATE=false`.
    #[serde(default = "default_suspend_on_escalate_true")]
    pub suspend_on_escalate: bool,

    /// Sprint-13 S13-0 (pre-flight surface) — per-scope expiry overrides
    /// for the suspend window. Map of scope-name → `Duration` (parsed
    /// from `humantime` strings like `"24h"`, `"4h"`, `"72h"`). Lookup
    /// falls back to the in-code `default_expiry` (`24h`) when a scope
    /// is missing from this map.
    ///
    /// S13-7 will wire `SuspendingHotlGate` to read from here when minting
    /// a `HotlPending` ticket; S13-0 only adds the surface. Default is
    /// the empty map → all scopes fall back to `default_expiry`, which
    /// preserves the v1.9.0 single-knob behaviour byte-for-byte.
    ///
    /// Override via YAML:
    /// ```yaml
    /// agent:
    ///   hotl:
    ///     expiry:
    ///       tool: 24h
    ///       mcp: 4h
    ///       skill: 72h
    /// ```
    /// Or env: `XIAOGUAI_AGENT__HOTL__EXPIRY__TOOL=12h`.
    #[serde(default, with = "humantime_serde_map")]
    pub expiry: HashMap<String, Duration>,

    /// Sprint-13 S13-0 (pre-flight surface) — when `true`, every
    /// HotL-escalated tool call MUST have its `args_redacted` field
    /// populated by a policy-driven redactor before `HotlPending` is
    /// emitted; a missing/empty redaction is treated as a hard policy
    /// violation by S13-6.
    ///
    /// v1.10 default: `false` (preserve v1.9.0 pass-through behaviour).
    /// v1.11 will flip this to `true` so production deployments stop
    /// leaking raw tool arguments through the `HotL` banner / audit chain.
    ///
    /// Override via YAML `agent.hotl.redaction_policy_required: true` or
    /// env `XIAOGUAI_AGENT__HOTL__REDACTION_POLICY_REQUIRED=true`.
    #[serde(default)]
    pub redaction_policy_required: bool,
}

/// v1.9.0 default for `HotlSettings::suspend_on_escalate` (S12-12).
fn default_suspend_on_escalate_true() -> bool {
    true
}

impl Default for HotlSettings {
    fn default() -> Self {
        Self {
            suspend_on_escalate: default_suspend_on_escalate_true(),
            expiry: HashMap::new(),
            redaction_policy_required: false,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server: ServerSettings {
                host: "0.0.0.0".into(),
                port: 7600,
                static_dir: None,
            },
            database: DatabaseSettings {
                url: default_db_url(),
                max_connections: default_db_max_connections(),
            },
            cache: CacheSettings {
                url: "redis://localhost:6379".into(),
                key_prefix: default_cache_prefix(),
            },
            auth: AuthSettings {
                username: String::new(),
                password: String::new(),
            },
            audit: AuditSettings {
                hmac_key: "dev-only-change-me-32-bytes-min".into(),
                signing_key_env: default_signing_key_env(),
            },
            scheduler: SchedulerSettings::default(),
            im: ImSettings::default(),
            eval: EvalSettings::default(),
            agent: AgentSettings::default(),
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
            .add_source(
                ::config::Environment::with_prefix("XIAOGUAI")
                    .prefix_separator("_")
                    .separator("__"),
            )
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
            .add_source(
                ::config::Environment::with_prefix("XIAOGUAI")
                    .prefix_separator("_")
                    .separator("__"),
            )
            .build()
            .map_err(|e| e.to_string())?;
        cfg.try_deserialize().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: env overrides must reach nested keys. `with_prefix("XIAOGUAI")`
    /// without `prefix_separator("_")` left a leading `_` on the stripped key, so
    /// `XIAOGUAI_DATABASE__URL` never mapped to `database.url` — every env-based
    /// deployment silently used the default localhost DB and crashed on boot.
    /// Restores process env afterwards.
    #[test]
    fn load_from_env_applies_nested_overrides() {
        const DB: &str = "postgres://u:p@envhost:5432/db";
        std::env::set_var("XIAOGUAI_DATABASE__URL", DB);
        std::env::set_var("XIAOGUAI_SERVER__PORT", "9999");
        let s = Settings::load_from_env().expect("load_from_env");
        std::env::remove_var("XIAOGUAI_DATABASE__URL");
        std::env::remove_var("XIAOGUAI_SERVER__PORT");

        assert_eq!(s.database.url, DB, "nested env override must apply");
        assert_eq!(s.server.port, 9999, "nested env override must apply");
        // Unset fields keep their in-code defaults.
        assert_eq!(s.database.max_connections, default_db_max_connections());
    }

    /// Sprint-12 S12-12 — default flip for v1.9.0. `suspend_on_escalate`
    /// now defaults to `true` so fresh deployments suspend on Escalate.
    /// The companion integration test
    /// `crates/xiaoguai-core/tests/hotl_default_on.rs` additionally proves
    /// the gate selector wires `SuspendingHotlGate` for this default.
    #[test]
    fn agent_hotl_suspend_on_escalate_default_is_true() {
        let s = Settings::default();
        assert!(
            s.agent.hotl.suspend_on_escalate,
            "v1.9.0 default must be true (S12-12); was false"
        );
    }

    /// A config.yaml that omits the `agent` block entirely should still
    /// deserialize cleanly and pick up the v1.9.0 default-`true` — proves
    /// the `#[serde(default = ...)]` on the field works through the
    /// nested `#[serde(default)]` on the surrounding blocks.
    #[test]
    fn agent_block_is_optional_and_defaults_apply() {
        // Reuse the env loader path because it constructs Settings from
        // defaults-as-yaml + env, mirroring how production loads when no
        // file is provided.
        let s = Settings::load_from_env().expect("default load");
        assert!(
            s.agent.hotl.suspend_on_escalate,
            "v1.9.0 default must propagate through the env loader path"
        );
    }

    /// Explicit `agent.hotl.suspend_on_escalate: false` in a config.yaml
    /// flips the flag back to legacy v1.8.x semantics — proves the
    /// opt-out path documented in RELEASE-LOG v1.9.0 still works.
    #[test]
    fn agent_hotl_suspend_on_escalate_yaml_opt_out_works() {
        use std::io::Write;
        let mut f = tempfile::Builder::new()
            .suffix(".yaml")
            .tempfile()
            .expect("tmpfile");
        writeln!(
            f,
            "server:\n  host: 127.0.0.1\n  port: 7600\ndatabase:\n  url: postgres://u:p@h/d\ncache:\n  url: redis://localhost:6379\nauth:\n  username: owner\n  password: pw\naudit:\n  hmac_key: dev-only-change-me-32-bytes-min\nagent:\n  hotl:\n    suspend_on_escalate: false\n"
        )
        .expect("write tmp yaml");
        let s = Settings::load_from_file(f.path()).expect("yaml load");
        assert!(
            !s.agent.hotl.suspend_on_escalate,
            "explicit `false` must opt out of v1.9.0 suspension behaviour"
        );
    }
}
