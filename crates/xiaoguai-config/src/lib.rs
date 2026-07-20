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

use serde::{Deserialize, Deserializer, Serialize};

/// serde adapter: accept a present-but-empty YAML key as an omitted one.
///
/// `#[serde(default)]` alone only fires when the key is *absent*. A block
/// header whose children are all commented out —
///
/// ```yaml
/// scheduler:
///   sinks:
///     # feishu:
///     #   webhook_url: ...
/// ```
///
/// — parses as `sinks: null`, which serde rejects with `invalid type: unit
/// value, expected struct SchedulerSinkSettings`. That shape is what the
/// shipped `config.example.yaml` looks like, so it bricked every rpm/deb
/// install at boot (the %post scriptlet seeds `config.yaml` from it).
/// Commenting out a block must degrade to "use the defaults", never to a
/// refuse-to-start.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// `Default` is derived: every block below now owns its own `Default`
/// impl, so the defaults live next to the fields they describe instead of
/// being duplicated in a hand-written `Settings::default()`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    /// The four blocks below stay *required* (a config.yaml with no
    /// `server:` key at all is still an error), but `null_as_default`
    /// makes a present-but-empty block fall back to its defaults instead
    /// of failing the load.
    #[serde(deserialize_with = "null_as_default")]
    pub server: ServerSettings,
    #[serde(deserialize_with = "null_as_default")]
    pub database: DatabaseSettings,
    #[serde(deserialize_with = "null_as_default")]
    pub auth: AuthSettings,
    #[serde(deserialize_with = "null_as_default")]
    pub audit: AuditSettings,
    /// Scheduler-side configuration. Optional so existing
    /// config.yaml files from v0.10.0/v0.10.1 still deserialize.
    /// v0.10.3 carries push-sink config under `scheduler.sinks.*`.
    #[serde(default, deserialize_with = "null_as_default")]
    pub scheduler: SchedulerSettings,
    /// v0.7.4: IM gateway runtime knobs.
    #[serde(default, deserialize_with = "null_as_default")]
    pub im: ImSettings,
    /// v0.11.2: eval pane substrate. Optional so existing
    /// config.yaml files still deserialize; defaults to disabled
    /// (`suites_dir = "./eval-suites"`, endpoints return 503 when
    /// the directory doesn't exist).
    #[serde(default, deserialize_with = "null_as_default")]
    pub eval: EvalSettings,
    /// Sprint-12 (S12-0): agent-loop runtime knobs. Optional so existing
    /// v1.8.x config.yaml files deserialize unchanged.
    #[serde(default, deserialize_with = "null_as_default")]
    pub agent: AgentSettings,
    /// DEC-036 (P1): long-term memory embedder selection. Optional so existing
    /// config files deserialize unchanged; the `OLLAMA_HOST` env still overrides.
    #[serde(default, deserialize_with = "null_as_default")]
    pub memory: MemorySettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSettings {
    /// `host` and `port` carry field-level defaults, not just the block-level
    /// `null_as_default`. An env override *inside* a block (the container image
    /// sets `XIAOGUAI_SERVER__HOST` and `__STATIC_DIR` but not `__PORT`) merges
    /// into the null block, so it is no longer null and `null_as_default` never
    /// fires — leaving a partial map that failed with `missing configuration
    /// field "server.port"`. Defaulting the fields closes that hole whatever
    /// the layering.
    #[serde(default = "default_server_host")]
    pub host: String,
    #[serde(default = "default_server_port")]
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

/// SEC-01: bind to loopback by default. Binding all interfaces (0.0.0.0) with
/// the auth gate disabled exposed the whole `/v1/**` surface to the
/// LAN/internet on a fresh `serve`. A safe default + an explicit
/// refuse-to-start guard (see `xiaoguai-core::run_serve`) replaces the old
/// warn-and-continue. Container images opt back into 0.0.0.0 via
/// `XIAOGUAI_SERVER__HOST` (set in the Dockerfile/compose).
fn default_server_host() -> String {
    "127.0.0.1".into()
}

const fn default_server_port() -> u16 {
    7600
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            host: default_server_host(),
            port: default_server_port(),
            static_dir: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseSettings {
    /// `SQLite` store location (DEC-033). A filesystem path or `sqlite://…` URL.
    /// Empty (the default) resolves to `$XDG_DATA_HOME/xiaoguai/data.db` or
    /// `~/.xiaoguai/data.db` — so a clean box can `serve` with no config.
    #[serde(default = "default_db_url")]
    pub url: String,
    #[serde(default = "default_db_max_connections")]
    pub max_connections: u32,
}

impl Default for DatabaseSettings {
    fn default() -> Self {
        Self {
            url: default_db_url(),
            max_connections: default_db_max_connections(),
        }
    }
}

/// Empty = resolve to the default per-user `SQLite` path at connect time.
fn default_db_url() -> String {
    String::new()
}

const fn default_db_max_connections() -> u32 {
    16
}

/// DEC-036 (P1): long-term memory configuration. Promotes the embedder choice
/// from the `OLLAMA_HOST` env to a documented config block. The env still
/// overrides at boot (back-compat), so existing air-gapped installs are
/// unaffected.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemorySettings {
    #[serde(default, deserialize_with = "null_as_default")]
    pub embedder: EmbedderSettings,
}

/// Which embedding backend long-term memory should use. `in_memory` (default)
/// is the deterministic, dependency-free embedder; `ollama` is the local,
/// air-gapped backend at `host`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EmbedderSettings {
    /// Local Ollama server (air-gapped semantic recall).
    Ollama {
        /// Base URL, e.g. `http://localhost:11434`.
        host: String,
    },
    /// Deterministic in-process embedder (no external service).
    #[default]
    InMemory,
}

/// Single-owner access gate (DEC-033). The API has no OIDC, RBAC, scopes,
/// or tenants — it is protected by one configured username + password
/// checked via HTTP Basic auth. When either field is empty the gate is
/// disabled and the server runs open (convenient for a localhost run);
/// front it with a credential before exposing it on a URL.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

/// The well-known dev/test audit HMAC key. It is intentionally public so
/// callers can detect "no real key configured" and refuse to sign a
/// production audit chain with it (SEC-15) — a chain signed with this key is
/// forgeable by anyone who reads the source.
pub const DEV_AUDIT_HMAC_KEY: &str = "dev-only-change-me-32-bytes-min";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditSettings {
    /// HMAC-SHA256 signing key for the audit chain. **NEVER** check in a real key.
    /// In production load via env or external secrets manager.
    ///
    /// Defaults to the well-known dev key for the same reason `server.host`
    /// does (see there): an env override landing elsewhere in this block would
    /// otherwise leave a partial map and fail the load. Defaulting is safe
    /// because the dev key is *detectable* — `xiaoguai-core` sources the
    /// production chain key from `signing_key_env` and never from here, and
    /// `commands::mod` refuses to sign with this value (SEC-15).
    #[serde(default = "default_audit_hmac_key")]
    pub hmac_key: String,
    /// v0.6.5: env-var name to read the production audit signing key from
    /// when wiring `SqliteAuditSink` in `xiaoguai-core`. The dev `hmac_key`
    /// above is fine for `smoke` and tests but must NOT be used for the
    /// production audit chain — operators set this knob and stash the
    /// real key in the named env var.
    #[serde(default = "default_signing_key_env")]
    pub signing_key_env: String,
}

/// SEC-15: the dev key is intentionally detectable — callers refuse to sign a
/// production audit chain with it.
fn default_audit_hmac_key() -> String {
    DEV_AUDIT_HMAC_KEY.into()
}

impl Default for AuditSettings {
    fn default() -> Self {
        Self {
            hmac_key: default_audit_hmac_key(),
            signing_key_env: default_signing_key_env(),
        }
    }
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
    /// operator wires only the sinks they actually deploy — including
    /// "none of them", which is what a `sinks:` header with all its
    /// children commented out means (see `null_as_default`).
    #[serde(default, deserialize_with = "null_as_default")]
    pub sinks: SchedulerSinkSettings,
    /// v0.12.2: filesystem-watch source bootstrap. Off by default so
    /// existing deployments keep the v0.12.0 webhook-only behaviour.
    /// Routes come from two places: the static `routes` list here
    /// (config-defined, ops-friendly) AND every persisted
    /// `scheduled_jobs` row whose `trigger.type == "file_watch"`
    /// (operator-friendly, edit a job in the admin pane to add a
    /// watch). Both lists merge into one [`FileWatchSource`].
    #[serde(default, deserialize_with = "null_as_default")]
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
    /// scenarios that shouldn't require a DB write. A present-but-null
    /// `routes:` means "none" rather than a parse error.
    #[serde(default, deserialize_with = "null_as_default")]
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
    #[serde(default, deserialize_with = "null_as_default")]
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
    ///
    /// A present-but-null `expiry:` yields an empty map rather than an error.
    /// The shipped example lists `tool`/`mcp`/`skill` under this key, and
    /// commenting all three out — the natural way to say "no per-scope
    /// overrides" — would otherwise reproduce the v1.34.0 boot failure
    /// verbatim. Same contract as the crate-level `null_as_default`, spelled
    /// separately because `with = "humantime_serde_map"` owns this field.
    pub fn deserialize<'de, D>(d: D) -> Result<HashMap<String, Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wrapped: Option<HashMap<String, humantime_serde::Serde<Duration>>> =
            Option::deserialize(d)?;
        Ok(wrapped
            .unwrap_or_default()
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

impl Settings {
    /// Load settings from a YAML file + environment overrides.
    ///
    /// Environment variables use the `XIAOGUAI_` prefix and `__` separator,
    /// e.g. `XIAOGUAI_DATABASE__URL=sqlite://...`.
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
    use std::io::Write;

    use super::*;

    /// Both loaders layer process env on top of their base, and env is
    /// process-global while cargo runs these tests as threads in ONE process.
    /// Without this lock, `load_from_env_applies_nested_overrides` setting
    /// `XIAOGUAI_SERVER__PORT=9999` leaks into whatever else is mid-load —
    /// measured 9 failures in 12 runs at `--test-threads=16`. Every test that
    /// reads or writes env must hold it.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Take [`ENV_LOCK`], ignoring poisoning: a panic in one test must fail
    /// that test, not cascade into every other one.
    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Regression: env overrides must reach nested keys. `with_prefix("XIAOGUAI")`
    /// without `prefix_separator("_")` left a leading `_` on the stripped key, so
    /// `XIAOGUAI_DATABASE__URL` never mapped to `database.url` — every env-based
    /// deployment silently used the default localhost DB and crashed on boot.
    /// Restores process env afterwards.
    #[test]
    fn load_from_env_applies_nested_overrides() {
        const DB: &str = "sqlite:///tmp/envhost.db";
        let _env = env_guard();
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
        let _env = env_guard();
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
        let _env = env_guard();
        let mut f = tempfile::Builder::new()
            .suffix(".yaml")
            .tempfile()
            .expect("tmpfile");
        writeln!(
            f,
            "server:\n  host: 127.0.0.1\n  port: 7600\ndatabase:\n  url: sqlite:///tmp/h.db\nauth:\n  username: owner\n  password: pw\naudit:\n  hmac_key: dev-only-change-me-32-bytes-min\nagent:\n  hotl:\n    suspend_on_escalate: false\n"
        )
        .expect("write tmp yaml");
        let s = Settings::load_from_file(f.path()).expect("yaml load");
        assert!(
            !s.agent.hotl.suspend_on_escalate,
            "explicit `false` must opt out of v1.9.0 suspension behaviour"
        );
    }

    /// Regression (v1.34.0 field report): the rpm/deb `%post` scriptlet seeds
    /// `/etc/xiaoguai/config.yaml` from this exact file, so if it fails to
    /// deserialize every packaged install crashes on boot in a systemd restart
    /// loop. Load the shipped example itself — not a hand-written copy — so the
    /// test can't drift away from what we actually ship.
    #[test]
    fn shipped_example_config_deserializes() {
        let _env = env_guard();
        let example = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../deploy/config.example.yaml")
            .canonicalize()
            .expect("deploy/config.example.yaml must exist");
        let s = Settings::load_from_file(&example)
            .unwrap_or_else(|e| panic!("shipped example config must load, got: {e}"));
        assert_eq!(
            s.server.port, 7600,
            "example must document the :7600 default"
        );

        // Deserializing is necessary but not sufficient: v1.34.0's example ALSO
        // paired `host: 0.0.0.0` with an empty auth block, which is exactly the
        // combination `run_serve`'s SEC-01 guard refuses — so a packaged install
        // that got past the parse error died on the very next check. Mirror that
        // guard here so the shipped example stays bootable as packaged.
        let loopback = s.server.host.eq_ignore_ascii_case("localhost")
            || s.server
                .host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback());
        assert!(
            loopback || s.auth.is_enabled(),
            "shipped example binds non-loopback `{}` with auth disabled — SEC-01 \
             makes `serve` refuse to start, so every packaged install would fail",
            s.server.host,
        );
    }

    /// A block header whose children are all commented out parses as null,
    /// which serde rejects with "invalid type: unit value, expected struct".
    /// That is precisely how `scheduler.sinks` bricked v1.34.0 rpm installs.
    /// Every optional block must degrade to its defaults instead.
    #[test]
    fn empty_nested_blocks_fall_back_to_defaults() {
        let _env = env_guard();
        let mut f = tempfile::Builder::new()
            .suffix(".yaml")
            .tempfile()
            .expect("tmpfile");
        // EVERY block is present-but-null here — the shape an operator produces
        // by commenting a block's children out. Includes the two the first cut
        // of this fix missed: `agent.hotl.expiry` (a map behind
        // `with = "humantime_serde_map"`) and `scheduler.file_watch.routes` (a
        // sequence), plus `server:`/`audit:`, whose sub-fields had no defaults.
        writeln!(
            f,
            "server:\ndatabase:\nauth:\naudit:\nscheduler:\n  enabled: false\n  sinks:\n  file_watch:\n    routes:\nagent:\n  hotl:\n    expiry:\nmemory:\n  embedder:\neval:\nim:\n"
        )
        .expect("write tmp yaml");

        let s = Settings::load_from_file(f.path())
            .unwrap_or_else(|e| panic!("null blocks must degrade to defaults, got: {e}"));

        assert!(s.scheduler.sinks.feishu.is_none(), "null sinks => no sinks");
        assert!(!s.scheduler.file_watch.enabled);
        assert!(
            s.scheduler.file_watch.routes.is_empty(),
            "null routes => none"
        );
        assert!(
            s.agent.hotl.suspend_on_escalate,
            "null hotl block must keep the v1.9.0 default-true"
        );
        assert!(
            s.agent.hotl.expiry.is_empty(),
            "null expiry => no overrides"
        );
        assert!(!s.auth.is_enabled(), "null auth block => gate disabled");
        assert_eq!(s.memory.embedder, EmbedderSettings::InMemory);
        // A null `server:`/`audit:` must land on the safe defaults, not on
        // whatever a partially-populated struct would have held.
        assert_eq!(s.server.host, "127.0.0.1");
        assert_eq!(s.server.port, 7600);
        assert_eq!(s.audit.hmac_key, DEV_AUDIT_HMAC_KEY);
        assert_eq!(s.eval.suites_dir, default_eval_suites_dir());
    }

    /// Regression for the gap the first cut of this fix left open: an env
    /// override landing *inside* a null block merges into it, so the block is
    /// no longer null and `null_as_default` never fires. The container image
    /// sets `XIAOGUAI_SERVER__HOST` + `__STATIC_DIR` but not `__PORT`, so a
    /// mounted config with `server:` commented out failed with `missing
    /// configuration field "server.port"`. Field-level defaults close it.
    #[test]
    fn env_override_into_a_null_block_still_loads() {
        let _env = env_guard();
        let mut f = tempfile::Builder::new()
            .suffix(".yaml")
            .tempfile()
            .expect("tmpfile");
        writeln!(f, "server:\ndatabase:\nauth:\naudit:\n").expect("write tmp yaml");

        std::env::set_var("XIAOGUAI_SERVER__HOST", "0.0.0.0");
        std::env::set_var("XIAOGUAI_SERVER__STATIC_DIR", "/app/static");
        let loaded = Settings::load_from_file(f.path());
        std::env::remove_var("XIAOGUAI_SERVER__HOST");
        std::env::remove_var("XIAOGUAI_SERVER__STATIC_DIR");

        let s = loaded.unwrap_or_else(|e| panic!("env-into-null-block must load, got: {e}"));
        assert_eq!(s.server.host, "0.0.0.0", "the env override must still win");
        assert_eq!(s.server.port, 7600, "the unset sibling must default");
        assert_eq!(s.server.static_dir.as_deref(), Some("/app/static"));
    }

    /// A block that is entirely ABSENT (not merely null) must still be a hard
    /// error for the four required blocks — `null_as_default` without
    /// `#[serde(default)]` tolerates null only. Guards against someone adding
    /// `default` to these fields and silently booting a misspelled config.
    #[test]
    fn missing_required_blocks_are_still_an_error() {
        let _env = env_guard();
        let mut f = tempfile::Builder::new()
            .suffix(".yaml")
            .tempfile()
            .expect("tmpfile");
        // `server:` deliberately absent.
        writeln!(f, "database:\nauth:\naudit:\n").expect("write tmp yaml");

        let err = Settings::load_from_file(f.path())
            .expect_err("a config.yaml with no `server:` key must not load");
        assert!(
            err.contains("server"),
            "error must name the missing block, got: {err}"
        );
    }
}
