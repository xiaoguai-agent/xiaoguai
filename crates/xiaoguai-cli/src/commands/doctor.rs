//! `xiaoguai doctor` — install/runtime self-check (T8.2,
//! `docs/plans/2026-06-10-install-polish.md` §1).
//!
//! Five checks, each producing a [`CheckResult`]:
//! 1. **database** — probe the resolved `SQLite` store **read-only** (#287):
//!    doctor never creates the file and never migrates (`sudo xiaoguai
//!    doctor` must not leave a root-owned store behind). A missing file is a
//!    `!` — `serve` creates and migrates it on first start; pending
//!    migrations are likewise a `!`.
//! 2. **providers** — the registry is non-empty and the default provider has
//!    the credentials it needs (Ollama needs none; cloud kinds need a stored
//!    key or a *present* `api_key_env`).
//! 3. **ollama** — when the default provider is an Ollama one: the endpoint
//!    answers `GET /api/tags` (2s timeout) and the default model is pulled.
//!    A missing model is a **WARN** (the server still boots; the first chat
//!    fails with a clear error) — only unreachability is a hard ✗.
//! 4. **port** — probe `GET /healthz` on the configured port: connection
//!    refused = free (✓ "not running"), HTTP 200 = already serving (✓ with a
//!    note), anything else = ✗ (a foreign process holds the port).
//! 5. **bind/auth** — SEC-01 parity (#287): `serve` refuses to start when
//!    binding a non-loopback host with owner auth disabled (unless
//!    `XIAOGUAI_ALLOW_UNAUTHENTICATED_NONLOOPBACK` opts out). Doctor mirrors
//!    that verdict so "all ✓" really means `serve` will boot.
//!
//! Classification fns are pure (unit-tested); the network/db layer is thin.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use xiaoguai_config::Settings;
use xiaoguai_storage::repositories::{LlmProviderRepository, SqliteLlmProviderRepository};
use xiaoguai_storage::{connect_read_only, pending_migration_count};
use xiaoguai_types::LlmProvider;

/// Network probe timeout — doctor must stay snappy even when nothing answers.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Outcome of one doctor check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    /// ✓ — healthy.
    Pass,
    /// ! — degraded but `serve` still boots (does NOT fail the exit code).
    Warn,
    /// ✗ — broken; doctor exits 1.
    Fail,
}

/// One row of the doctor report.
#[derive(Debug, Clone)]
pub struct CheckResult {
    pub name: &'static str,
    pub status: CheckStatus,
    pub detail: String,
}

impl CheckResult {
    fn pass(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: CheckStatus::Pass,
            detail: detail.into(),
        }
    }
    fn warn(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: CheckStatus::Warn,
            detail: detail.into(),
        }
    }
    fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: CheckStatus::Fail,
            detail: detail.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Pure classification / formatting
// ---------------------------------------------------------------------------

/// Render the ✓/!/✗ table. Pure.
#[must_use]
pub fn format_report(results: &[CheckResult]) -> String {
    let mut out = String::new();
    for r in results {
        let mark = match r.status {
            CheckStatus::Pass => "✓",
            CheckStatus::Warn => "!",
            CheckStatus::Fail => "✗",
        };
        let _ = writeln!(
            out,
            "{mark} {name:<12} {detail}",
            name = r.name,
            detail = r.detail
        );
    }
    out
}

/// True when any check is a hard ✗ (warns do not count). Pure.
#[must_use]
pub fn has_failure(results: &[CheckResult]) -> bool {
    results.iter().any(|r| r.status == CheckStatus::Fail)
}

/// The provider `serve` treats as primary: lowest `fallback_order`, ties
/// broken by `created_at` — same sort the `LlmRouter` applies. Pure.
#[must_use]
pub fn pick_default_provider(rows: &[LlmProvider]) -> Option<&LlmProvider> {
    rows.iter().min_by_key(|p| (p.fallback_order, p.created_at))
}

/// Classify the default provider's credential posture. Pure: the caller
/// resolves whether `api_key_env` is actually present and passes it in as
/// `env_present`.
#[must_use]
pub fn classify_provider_key(provider: &LlmProvider, env_present: Option<bool>) -> CheckResult {
    let name = &provider.name;
    if provider.kind.as_str() == "ollama" {
        return CheckResult::pass(
            "providers",
            format!("default: {name} (local — no API key needed)"),
        );
    }
    if provider.api_key.is_some() {
        return CheckResult::pass("providers", format!("default: {name} (API key stored)"));
    }
    match (&provider.api_key_env, env_present) {
        (Some(var), Some(true)) => {
            CheckResult::pass("providers", format!("default: {name} (key via ${var})"))
        }
        (Some(var), _) => CheckResult::fail(
            "providers",
            format!(
                "default {name} reads ${var}, but it is not set — export it or run: xiaoguai init"
            ),
        ),
        (None, _) => CheckResult::fail(
            "providers",
            format!("default {name} has no API key — run: xiaoguai init"),
        ),
    }
}

/// Classify the Ollama model inventory against the model `serve` will use.
/// `available` is the `name` list from `GET /api/tags` (entries usually carry
/// a `:tag` suffix, e.g. `qwen2.5-coder:latest`). Missing model = WARN — the
/// server boots; only the first chat would fail. Pure.
#[must_use]
pub fn classify_ollama_models(available: &[String], wanted: &str) -> CheckResult {
    if wanted.is_empty() {
        return CheckResult::pass(
            "ollama",
            "reachable (no default model configured to verify)",
        );
    }
    let found = available
        .iter()
        .any(|m| m == wanted || m.split(':').next() == Some(wanted));
    if found {
        CheckResult::pass("ollama", format!("reachable; model {wanted} is pulled"))
    } else {
        CheckResult::warn(
            "ollama",
            format!("reachable, but model {wanted} is not pulled — run: ollama pull {wanted}"),
        )
    }
}

/// Raw outcome of probing `GET /healthz` on the configured port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortProbe {
    /// TCP connection refused — nothing listens; the port is free.
    Refused,
    /// HTTP 200 from `/healthz` — a xiaoguai server already runs here.
    Healthy,
    /// Something answered, but not a healthy xiaoguai (`/healthz` ≠ 200).
    UnexpectedStatus(u16),
    /// Any other transport error (timeout, reset, DNS, …).
    OtherError(String),
}

/// Map a [`PortProbe`] to a check row. Free and already-serving are both ✓;
/// a foreign occupant or an undiagnosable error is ✗. Pure.
#[must_use]
pub fn classify_port_probe(probe: &PortProbe, port: u16) -> CheckResult {
    match probe {
        PortProbe::Refused => {
            CheckResult::pass("port", format!("{port} is free (no server running)"))
        }
        PortProbe::Healthy => CheckResult::pass(
            "port",
            format!("a xiaoguai server is already serving on {port} (healthz ok)"),
        ),
        PortProbe::UnexpectedStatus(code) => CheckResult::fail(
            "port",
            format!("{port} is held by something that is not a healthy xiaoguai (healthz → HTTP {code}) — try: lsof -i :{port}"),
        ),
        PortProbe::OtherError(e) => CheckResult::fail(
            "port",
            format!("probing {port} failed: {e} — try: lsof -i :{port}"),
        ),
    }
}

/// The model `serve` would treat as the deployment default for `provider` —
/// first `default_for_models` entry, else the first declared model. Pure.
#[must_use]
pub fn default_model_of(provider: &LlmProvider) -> String {
    provider
        .default_for_models
        .first()
        .or_else(|| provider.models.first())
        .cloned()
        .unwrap_or_default()
}

/// SEC-01 parity (#287): is `host` a loopback bind (safe to run
/// unauthenticated)? `localhost`, `127.0.0.0/8`, and `::1` are loopback;
/// `0.0.0.0` / `::` (all interfaces) and any routable address are not. An
/// unparseable hostname is treated as non-loopback (fail-safe).
///
/// This is an intentional copy of the private `host_is_loopback` in
/// `xiaoguai-core/src/lib.rs` — keep the two in sync. Pure.
#[must_use]
pub fn host_is_loopback(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

/// SEC-01 parity (#287): same truthy parsing `xiaoguai-core` applies to
/// `XIAOGUAI_ALLOW_UNAUTHENTICATED_NONLOOPBACK`. Pure: the caller reads the
/// env var and passes the raw value in.
#[must_use]
pub fn is_truthy_override(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Classify the bind/auth posture against the SEC-01 startup gate (#287).
///
/// `serve` refuses to boot when binding a non-loopback host with owner auth
/// disabled and no explicit override — doctor must report ✗ for exactly that
/// combination (it used to report all-✓ and `serve` then refused). With the
/// override set, `serve` boots but the `/v1` API is exposed → `!`. Pure.
#[must_use]
pub fn classify_bind_auth(host: &str, auth_enabled: bool, override_set: bool) -> CheckResult {
    if host_is_loopback(host) {
        return CheckResult::pass(
            "bind/auth",
            format!("loopback bind ({host}) — safe with or without owner auth"),
        );
    }
    if auth_enabled {
        return CheckResult::pass(
            "bind/auth",
            format!("non-loopback bind ({host}) with owner auth enabled"),
        );
    }
    if override_set {
        return CheckResult::warn(
            "bind/auth",
            format!(
                "non-loopback bind ({host}) with owner auth DISABLED — serve boots only because \
                 XIAOGUAI_ALLOW_UNAUTHENTICATED_NONLOOPBACK is set; the /v1 API is exposed \
                 unauthenticated, make sure a trusted reverse proxy fronts it"
            ),
        );
    }
    CheckResult::fail(
        "bind/auth",
        format!(
            "serve will REFUSE to start (SEC-01): non-loopback bind ({host}) with owner auth \
             disabled. Fix one of: \
             1) set auth.username + auth.password (XIAOGUAI_AUTH__USERNAME / __PASSWORD); \
             2) bind loopback (XIAOGUAI_SERVER__HOST=127.0.0.1); \
             3) (NOT recommended — only behind a trusted proxy) set \
             XIAOGUAI_ALLOW_UNAUTHENTICATED_NONLOOPBACK=1"
        ),
    )
}

/// Database row for a store file that does not exist yet (#287). Doctor must
/// not create it — `serve` does that on first start, so this is a `!`, not a
/// ✗. Pure.
#[must_use]
pub fn classify_missing_db(db_path: &Path) -> CheckResult {
    CheckResult::warn(
        "database",
        format!(
            "not created yet ({}) — `xiaoguai serve` creates and migrates it on first start",
            db_path.display()
        ),
    )
}

/// Database row for an existing, readable store given its pending-migration
/// count (#287). Zero pending = ✓; pending migrations = `!` (serve applies
/// them on next start; doctor never migrates). Pure.
#[must_use]
pub fn classify_schema_state(pending: usize, db_path: &Path) -> CheckResult {
    if pending == 0 {
        CheckResult::pass(
            "database",
            format!("readable, schema current ({})", db_path.display()),
        )
    } else {
        CheckResult::warn(
            "database",
            format!(
                "readable, but {pending} migration(s) pending ({}) — `xiaoguai serve` applies \
                 them on next start",
                db_path.display()
            ),
        )
    }
}

// ---------------------------------------------------------------------------
// Impure layer: db open, registry read, network probes
// ---------------------------------------------------------------------------

/// Run all checks against `settings` and return the report rows.
///
/// Never returns `Err` for a *failing* check — failures are rows; only
/// caller-side printing decides the exit code via [`has_failure`].
pub async fn run(settings: &Settings) -> Vec<CheckResult> {
    let mut results = Vec::new();

    // 1. database — read-only probe (#287). Doctor must never create the
    // file or run migrations: `sudo xiaoguai doctor` used to leave a
    // root-owned, forward-migrated store behind and break the next `serve`.
    let db_path = super::backup::resolve_sqlite_path(&settings.database.url);
    let db_missing = !db_path.exists();
    let pool = if db_missing {
        results.push(classify_missing_db(&db_path));
        None
    } else {
        match connect_read_only(&settings.database.url).await {
            Ok(pool) => match pending_migration_count(&pool).await {
                Ok(pending) => {
                    results.push(classify_schema_state(pending, &db_path));
                    // Schema behind → registry tables may not exist yet;
                    // skip the registry-backed checks rather than ✗ on them.
                    (pending == 0).then_some(pool)
                }
                Err(e) => {
                    results.push(CheckResult::fail(
                        "database",
                        format!("schema check failed on {}: {e:#}", db_path.display()),
                    ));
                    None
                }
            },
            Err(e) => {
                results.push(CheckResult::fail(
                    "database",
                    format!("cannot open {} read-only: {e:#}", db_path.display()),
                ));
                None
            }
        }
    };

    // 2 + 3. providers / ollama — need the registry, so they ride on the pool.
    let mut ollama_target: Option<(String, String)> = None; // (endpoint, model)
    if let Some(pool) = pool {
        let repo = SqliteLlmProviderRepository::new(pool);
        match repo.list().await {
            Ok(rows) => match pick_default_provider(&rows) {
                Some(default) => {
                    let env_present = default
                        .api_key_env
                        .as_deref()
                        .map(|var| std::env::var(var).is_ok_and(|v| !v.trim().is_empty()));
                    results.push(classify_provider_key(default, env_present));
                    if default.kind.as_str() == "ollama" {
                        // Mirror serve's OLLAMA_HOST override for the seeded
                        // ollama-local row (see run_serve).
                        let mut endpoint = default.endpoint.clone();
                        if default.id.as_str() == "ollama-local" {
                            if let Ok(host) = std::env::var("OLLAMA_HOST") {
                                if !host.trim().is_empty() {
                                    endpoint = host.trim().to_string();
                                }
                            }
                        }
                        ollama_target = Some((endpoint, default_model_of(default)));
                    }
                }
                None => results.push(CheckResult::fail(
                    "providers",
                    "no providers configured — run `xiaoguai serve` once (seeds defaults), then: xiaoguai init",
                )),
            },
            Err(e) => results.push(CheckResult::fail("providers", format!("registry read failed: {e:#}"))),
        }
    } else if db_missing {
        // Not a ✗: serve seeds default providers when it creates the store.
        // (#287)
        results.push(CheckResult::warn(
            "providers",
            "skipped — database not created yet (`xiaoguai serve` seeds defaults on first start)",
        ));
    } else if has_failure(&results) {
        results.push(CheckResult::fail(
            "providers",
            "skipped — database unavailable",
        ));
    } else {
        // Readable store with pending migrations — registry tables may lag.
        // (#287)
        results.push(CheckResult::warn(
            "providers",
            "skipped — schema not current; re-run doctor after `xiaoguai serve` migrates",
        ));
    }

    if let Some((endpoint, model)) = ollama_target {
        results.push(check_ollama(&endpoint, &model).await);
    }

    // 4. port — healthz probe on the configured serve address.
    let probe = probe_port(&settings.server.host, settings.server.port).await;
    results.push(classify_port_probe(&probe, settings.server.port));

    // 5. bind/auth posture — SEC-01 parity (#287): mirror the startup gate so
    // doctor cannot report all-✓ for a config `serve` will refuse to boot.
    let override_set = std::env::var("XIAOGUAI_ALLOW_UNAUTHENTICATED_NONLOOPBACK")
        .is_ok_and(|v| is_truthy_override(&v));
    results.push(classify_bind_auth(
        &settings.server.host,
        settings.auth.is_enabled(),
        override_set,
    ));

    results
}

/// GET `<endpoint>/api/tags` and classify reachability + model presence.
async fn check_ollama(endpoint: &str, model: &str) -> CheckResult {
    #[derive(serde::Deserialize)]
    struct Tags {
        #[serde(default)]
        models: Vec<TagEntry>,
    }
    #[derive(serde::Deserialize)]
    struct TagEntry {
        name: String,
    }

    let url = format!("{}/api/tags", endpoint.trim_end_matches('/'));
    let resp = match probe_client() {
        Ok(client) => client.get(&url).send().await,
        Err(e) => return CheckResult::fail("ollama", format!("probe setup failed: {e:#}")),
    };
    match resp {
        Ok(r) if r.status().is_success() => match r.json::<Tags>().await {
            Ok(tags) => {
                let names: Vec<String> = tags.models.into_iter().map(|m| m.name).collect();
                classify_ollama_models(&names, model)
            }
            Err(e) => CheckResult::fail(
                "ollama",
                format!("{url} answered but the tag list is unreadable: {e}"),
            ),
        },
        Ok(r) => CheckResult::fail(
            "ollama",
            format!("{url} → HTTP {} — is that really an Ollama endpoint?", r.status()),
        ),
        Err(e) => CheckResult::fail(
            "ollama",
            format!(
                "unreachable at {endpoint} ({e}) — start it with `ollama serve`, or repoint via OLLAMA_HOST"
            ),
        ),
    }
}

/// Probe `GET /healthz` on the configured serve address.
async fn probe_port(host: &str, port: u16) -> PortProbe {
    // A wildcard bind host isn't a dialable address — probe loopback instead.
    let dial_host = match host.trim() {
        "" | "0.0.0.0" | "::" => "127.0.0.1",
        h => h,
    };
    let url = format!("http://{dial_host}:{port}/healthz");
    let client = match probe_client() {
        Ok(c) => c,
        Err(e) => return PortProbe::OtherError(format!("{e:#}")),
    };
    match client.get(&url).send().await {
        Ok(r) if r.status().as_u16() == 200 => PortProbe::Healthy,
        Ok(r) => PortProbe::UnexpectedStatus(r.status().as_u16()),
        Err(e) if e.is_connect() => PortProbe::Refused,
        Err(e) => PortProbe::OtherError(e.to_string()),
    }
}

fn probe_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use xiaoguai_types::{LlmProvider, ProviderId, ProviderKind};

    fn prov(
        name: &str,
        kind: ProviderKind,
        fallback_order: i32,
        api_key: Option<&str>,
        api_key_env: Option<&str>,
    ) -> LlmProvider {
        let now = Utc::now();
        LlmProvider {
            id: ProviderId::new(),
            name: name.into(),
            kind,
            endpoint: "http://localhost:11434".into(),
            models: vec!["qwen2.5-coder".into()],
            default_for_models: vec![],
            fallback_order,
            api_key_env: api_key_env.map(Into::into),
            api_key: api_key.map(Into::into),
            created_at: now,
            updated_at: now,
            cost_per_1k_input_usd: None,
            cost_per_1k_output_usd: None,
        }
    }

    #[test]
    fn default_provider_is_lowest_fallback_order() {
        let rows = vec![
            prov("cloud", ProviderKind::OpenAiCompat, 1, Some("k"), None),
            prov("local", ProviderKind::Ollama, 0, None, None),
        ];
        assert_eq!(pick_default_provider(&rows).unwrap().name, "local");
        assert!(pick_default_provider(&[]).is_none());
    }

    #[test]
    fn ollama_default_needs_no_key() {
        let p = prov("Ollama (local)", ProviderKind::Ollama, 0, None, None);
        let r = classify_provider_key(&p, None);
        assert_eq!(r.status, CheckStatus::Pass);
        assert!(r.detail.contains("no API key needed"));
    }

    #[test]
    fn cloud_default_passes_with_stored_key_or_present_env() {
        let stored = prov("MiniMax", ProviderKind::MiniMax, 0, Some("sk-x"), None);
        assert_eq!(
            classify_provider_key(&stored, None).status,
            CheckStatus::Pass
        );

        let via_env = prov("MiniMax", ProviderKind::MiniMax, 0, None, Some("MM_KEY"));
        assert_eq!(
            classify_provider_key(&via_env, Some(true)).status,
            CheckStatus::Pass
        );
    }

    #[test]
    fn cloud_default_fails_without_any_key() {
        let keyless = prov("MiniMax", ProviderKind::MiniMax, 0, None, None);
        let r = classify_provider_key(&keyless, None);
        assert_eq!(r.status, CheckStatus::Fail);
        assert!(r.detail.contains("xiaoguai init"));

        let env_missing = prov("MiniMax", ProviderKind::MiniMax, 0, None, Some("MM_KEY"));
        let r = classify_provider_key(&env_missing, Some(false));
        assert_eq!(r.status, CheckStatus::Fail);
        assert!(r.detail.contains("$MM_KEY"));
    }

    #[test]
    fn ollama_model_match_ignores_tag_suffix() {
        let avail = vec!["qwen2.5-coder:latest".to_string(), "llama3:8b".to_string()];
        let r = classify_ollama_models(&avail, "qwen2.5-coder");
        assert_eq!(r.status, CheckStatus::Pass);
    }

    #[test]
    fn ollama_missing_model_is_a_warn_with_pull_hint() {
        let r = classify_ollama_models(&[], "qwen2.5-coder");
        assert_eq!(r.status, CheckStatus::Warn);
        assert!(r.detail.contains("ollama pull qwen2.5-coder"));
    }

    #[test]
    fn port_probe_classification() {
        assert_eq!(
            classify_port_probe(&PortProbe::Refused, 7600).status,
            CheckStatus::Pass
        );
        let healthy = classify_port_probe(&PortProbe::Healthy, 7600);
        assert_eq!(healthy.status, CheckStatus::Pass);
        assert!(healthy.detail.contains("already serving"));
        assert_eq!(
            classify_port_probe(&PortProbe::UnexpectedStatus(404), 7600).status,
            CheckStatus::Fail
        );
        assert_eq!(
            classify_port_probe(&PortProbe::OtherError("reset".into()), 7600).status,
            CheckStatus::Fail
        );
    }

    #[test]
    fn report_marks_and_exit_semantics() {
        let results = vec![
            CheckResult::pass("database", "ok"),
            CheckResult::warn("ollama", "model missing"),
        ];
        let table = format_report(&results);
        assert!(table.contains("✓ database"));
        assert!(table.contains("! ollama"));
        assert!(!has_failure(&results), "warns must not fail the exit code");

        let with_fail = vec![CheckResult::fail("port", "held")];
        assert!(format_report(&with_fail).contains("✗ port"));
        assert!(has_failure(&with_fail));
    }

    // ---- #287: bind/auth posture (SEC-01 parity) ----

    #[test]
    fn loopback_hosts_are_recognised() {
        for host in ["localhost", "LOCALHOST", "127.0.0.1", "127.8.9.10", "::1"] {
            assert!(host_is_loopback(host), "{host} must count as loopback");
        }
        // Wildcard binds expose all interfaces; unparseable names fail safe.
        for host in [
            "0.0.0.0",
            "::",
            "192.168.1.5",
            "10.0.0.1",
            "example.com",
            "",
        ] {
            assert!(!host_is_loopback(host), "{host} must NOT count as loopback");
        }
    }

    #[test]
    fn nonloopback_without_auth_fails_with_sec01_remediation() {
        let r = classify_bind_auth("0.0.0.0", false, false);
        assert_eq!(r.status, CheckStatus::Fail);
        assert!(r.detail.contains("SEC-01"));
        assert!(r.detail.contains("REFUSE to start"));
        // The three remediations must match the serve-side bail message.
        assert!(r.detail.contains("auth.username + auth.password"));
        assert!(r.detail.contains("XIAOGUAI_SERVER__HOST=127.0.0.1"));
        assert!(r
            .detail
            .contains("XIAOGUAI_ALLOW_UNAUTHENTICATED_NONLOOPBACK=1"));
    }

    #[test]
    fn loopback_or_authed_binds_pass() {
        assert_eq!(
            classify_bind_auth("127.0.0.1", false, false).status,
            CheckStatus::Pass
        );
        assert_eq!(
            classify_bind_auth("localhost", false, false).status,
            CheckStatus::Pass
        );
        assert_eq!(
            classify_bind_auth("0.0.0.0", true, false).status,
            CheckStatus::Pass
        );
    }

    #[test]
    fn nonloopback_without_auth_but_override_is_a_warn() {
        let r = classify_bind_auth("0.0.0.0", false, true);
        assert_eq!(r.status, CheckStatus::Warn);
        assert!(r
            .detail
            .contains("XIAOGUAI_ALLOW_UNAUTHENTICATED_NONLOOPBACK"));
    }

    #[test]
    fn override_truthy_parsing_matches_core() {
        for v in ["1", "true", "YES", " on "] {
            assert!(is_truthy_override(v), "{v:?} must be truthy");
        }
        for v in ["", "0", "false", "off", "nope"] {
            assert!(!is_truthy_override(v), "{v:?} must be falsy");
        }
    }

    // ---- #287: read-only database probe classification ----

    #[test]
    fn missing_db_is_a_warn_not_a_fail() {
        let r = classify_missing_db(Path::new("/home/me/.xiaoguai/data.db"));
        assert_eq!(r.status, CheckStatus::Warn, "doctor must not create the DB");
        assert!(r.detail.contains("not created yet"));
        assert!(r.detail.contains("serve"));
        assert!(r.detail.contains("/home/me/.xiaoguai/data.db"));
    }

    #[test]
    fn schema_state_classification() {
        let path = Path::new("/tmp/data.db");
        let current = classify_schema_state(0, path);
        assert_eq!(current.status, CheckStatus::Pass);
        assert!(current.detail.contains("schema current"));

        let behind = classify_schema_state(3, path);
        assert_eq!(behind.status, CheckStatus::Warn, "doctor must not migrate");
        assert!(behind.detail.contains("3 migration(s) pending"));
        assert!(behind.detail.contains("serve"));
    }

    #[test]
    fn default_model_prefers_default_for_models() {
        let mut p = prov("o", ProviderKind::Ollama, 0, None, None);
        assert_eq!(default_model_of(&p), "qwen2.5-coder");
        p.default_for_models = vec!["other".into()];
        assert_eq!(default_model_of(&p), "other");
        p.models.clear();
        p.default_for_models.clear();
        assert_eq!(default_model_of(&p), "");
    }
}
