//! Sprint-13 S13-0 — pre-flight config surface for per-scope expiry +
//! redaction-policy-required.
//!
//! These tests pin the config-shape contract that S13-7 (per-scope expiry
//! lookup) and S13-6 (redaction policy enforcement) will wire into the
//! HotL gate. S13-0 only adds the surface — defaults preserve v1.9.0
//! behaviour (empty map → fall back to `default_expiry`; redaction policy
//! NOT required).
//!
//! The tests live in `xiaoguai-core` (rather than `xiaoguai-config`) so we
//! exercise the full `Settings::load_from_file` path that production code
//! uses. Mirrors the `hotl_default_on.rs` test layout from S12-12.
//!
//! Per S13-0 task brief: this file is committed RED first (fields missing
//! on `HotlSettings`), then the impl commit adds:
//!
//!   - `pub expiry: HashMap<String, Duration>` (default empty)
//!   - `pub redaction_policy_required: bool` (default false)
//!
//! Test fixtures use the `humantime` string form (`"24h"`) because that
//! matches the documented YAML surface in `deploy/config.example.yaml`.
//!
//! ## Env override — deliberately not asserted here
//!
//! The task brief mentioned env override via
//! `XIAOGUAI_AGENT__HOTL__EXPIRY__TOOL=12h`. While the loader chain
//! (`config::Environment::with_prefix("XIAOGUAI").separator("__")`)
//! claims to nest `__` into table paths, in practice v0.15 of the
//! `config` crate does NOT promote a flat env key like
//! `XIAOGUAI_AGENT__HOTL__EXPIRY__TOOL` into a `HashMap` leaf — the
//! resulting `expiry` map ends up empty. This same limitation affects
//! the existing `XIAOGUAI_AGENT__HOTL__SUSPEND_ON_ESCALATE` knob (the
//! S12-12 docs claim env override works but no test pinned it). Fixing
//! the loader is a separate scope (a candidate S13 carry-forward); S13-0
//! is documented as "config surface, no code-path changes" so we pin
//! only the YAML + defaults paths here. Operators relying on env-only
//! injection of `agent.hotl.expiry` should set the YAML file too.

use std::io::Write;
use std::time::Duration;

use xiaoguai_config::Settings;

/// YAML round-trip: an explicit `agent.hotl.expiry.tool: 24h` block must
/// parse into a `Duration` of 24 hours under the `tool` key. `mcp` and
/// `skill` keys should round-trip independently. This pins the
/// per-scope-expiry surface that S13-7 will read from.
#[test]
fn yaml_per_scope_expiry_parses_humantime_strings() {
    let mut f = tempfile::Builder::new()
        .suffix(".yaml")
        .tempfile()
        .expect("tmpfile");
    writeln!(
        f,
        "server:\n  host: 127.0.0.1\n  port: 7600\ndatabase:\n  url: postgres://u:p@h/d\ncache:\n  url: redis://localhost:6379\nauth:\n  issuer: a\n  audience: b\n  jwks_url: c\naudit:\n  hmac_key: dev-only-change-me-32-bytes-min\nagent:\n  hotl:\n    expiry:\n      tool: 24h\n      mcp: 4h\n      skill: 72h\n"
    )
    .expect("write tmp yaml");

    let s = Settings::load_from_file(f.path()).expect("yaml load");

    assert_eq!(
        s.agent.hotl.expiry.get("tool").copied(),
        Some(Duration::from_secs(24 * 3600)),
        "tool scope must parse 24h"
    );
    assert_eq!(
        s.agent.hotl.expiry.get("mcp").copied(),
        Some(Duration::from_secs(4 * 3600)),
        "mcp scope must parse 4h"
    );
    assert_eq!(
        s.agent.hotl.expiry.get("skill").copied(),
        Some(Duration::from_secs(72 * 3600)),
        "skill scope must parse 72h"
    );
}

/// `redaction_policy_required: true` in YAML must surface on
/// `HotlSettings.redaction_policy_required`. This pins the flag that
/// S13-6 will read to gate `HotlPending.args_redacted` policy enforcement.
#[test]
fn yaml_redaction_policy_required_parses_true() {
    let mut f = tempfile::Builder::new()
        .suffix(".yaml")
        .tempfile()
        .expect("tmpfile");
    writeln!(
        f,
        "server:\n  host: 127.0.0.1\n  port: 7600\ndatabase:\n  url: postgres://u:p@h/d\ncache:\n  url: redis://localhost:6379\nauth:\n  issuer: a\n  audience: b\n  jwks_url: c\naudit:\n  hmac_key: dev-only-change-me-32-bytes-min\nagent:\n  hotl:\n    redaction_policy_required: true\n"
    )
    .expect("write tmp yaml");

    let s = Settings::load_from_file(f.path()).expect("yaml load");
    assert!(
        s.agent.hotl.redaction_policy_required,
        "explicit `true` in YAML must flip redaction_policy_required"
    );
}

/// Default surface: omitting both new keys must yield an empty `expiry`
/// map and `redaction_policy_required == false`. v1.9.x backwards-compat
/// requirement — a config without the new block must keep behaving
/// exactly as v1.9.0 did.
#[test]
fn defaults_preserve_v190_behaviour() {
    let s = Settings::default();
    assert!(
        s.agent.hotl.expiry.is_empty(),
        "default expiry map must be empty (fall back to default_expiry)"
    );
    assert!(
        !s.agent.hotl.redaction_policy_required,
        "default redaction_policy_required must be false (v1.11 will flip)"
    );
}

/// YAML omitting both new keys (but providing the rest of the config)
/// also yields the v1.9.0-compatible defaults — proves the
/// `#[serde(default)]` annotations on each field survive the loader.
#[test]
fn yaml_without_new_keys_preserves_v190_behaviour() {
    let mut f = tempfile::Builder::new()
        .suffix(".yaml")
        .tempfile()
        .expect("tmpfile");
    writeln!(
        f,
        "server:\n  host: 127.0.0.1\n  port: 7600\ndatabase:\n  url: postgres://u:p@h/d\ncache:\n  url: redis://localhost:6379\nauth:\n  issuer: a\n  audience: b\n  jwks_url: c\naudit:\n  hmac_key: dev-only-change-me-32-bytes-min\nagent:\n  hotl:\n    suspend_on_escalate: true\n"
    )
    .expect("write tmp yaml");

    let s = Settings::load_from_file(f.path()).expect("yaml load");
    assert!(
        s.agent.hotl.expiry.is_empty(),
        "absent expiry block must yield empty map"
    );
    assert!(
        !s.agent.hotl.redaction_policy_required,
        "absent redaction_policy_required must default to false"
    );
}
