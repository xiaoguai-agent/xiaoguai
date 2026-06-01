//! Sprint-14 S14-0 — pre-flight config surface for boot-replay pagination.
//!
//! Pins the YAML + default + validation contract for the new
//! `agent.hotl.replay_page_size` key. S14-7 will consume this knob to
//! page the boot-time HotL escalation replay; S14-0 only adds the
//! config surface so the validation contract is locked before any
//! runtime code reads from it.
//!
//! Layout mirrors `config_per_scope_expiry.rs` (S13-0) so the
//! sprint-13 → sprint-14 config-surface evolution stays consistent.
//!
//! Contract:
//!   - `pub replay_page_size: usize`
//!   - default value: 256 (when key omitted from YAML)
//!   - validation: `>= 1`; deserialising `0` must fail with an error
//!     mentioning the field name so operators can pinpoint the bad key
//!
//! Per S13-0's precedent, env-override coverage is deferred — the
//! `config::Environment` loader has documented limitations around
//! scalar leaves under nested tables; the YAML + default paths are
//! the contract that matters for the boot-replay consumer in S14-7.

use std::io::Write;

use xiaoguai_config::Settings;

const YAML_PREFIX: &str = "server:\n  host: 127.0.0.1\n  port: 7600\ndatabase:\n  url: postgres://u:p@h/d\ncache:\n  url: redis://localhost:6379\nauth:\n  issuer: a\n  audience: b\n  jwks_url: c\naudit:\n  hmac_key: dev-only-change-me-32-bytes-min\n";

/// YAML round-trip: an explicit `agent.hotl.replay_page_size: 512`
/// must surface on `HotlSettings.replay_page_size`. Pins the
/// happy-path read for S14-7.
#[test]
fn yaml_replay_page_size_parses_explicit_value() {
    let mut f = tempfile::Builder::new()
        .suffix(".yaml")
        .tempfile()
        .expect("tmpfile");
    write!(
        f,
        "{YAML_PREFIX}agent:\n  hotl:\n    replay_page_size: 512\n"
    )
    .expect("write tmp yaml");

    let s = Settings::load_from_file(f.path()).expect("yaml load");
    assert_eq!(
        s.agent.hotl.replay_page_size, 512,
        "explicit replay_page_size must round-trip from YAML"
    );
}

/// Default surface: omitting the key from YAML yields `256`. v1.10.x
/// backwards-compat requirement — a config without the new key must
/// deserialize without complaint and pick up the in-code default.
#[test]
fn yaml_without_replay_page_size_uses_default_256() {
    let mut f = tempfile::Builder::new()
        .suffix(".yaml")
        .tempfile()
        .expect("tmpfile");
    write!(
        f,
        "{YAML_PREFIX}agent:\n  hotl:\n    suspend_on_escalate: true\n"
    )
    .expect("write tmp yaml");

    let s = Settings::load_from_file(f.path()).expect("yaml load");
    assert_eq!(
        s.agent.hotl.replay_page_size, 256,
        "absent replay_page_size must default to 256"
    );
}

/// `Settings::default()` (constructed in-code, no YAML) must also
/// land on `256` so consumers that build a `Settings` for tests get
/// the same boot-replay paging behaviour as a fresh deployment.
#[test]
fn default_settings_replay_page_size_is_256() {
    let s = Settings::default();
    assert_eq!(
        s.agent.hotl.replay_page_size, 256,
        "Settings::default() must seed replay_page_size with 256"
    );
}

/// Validation: `replay_page_size: 0` must be rejected at load time
/// with an error message that mentions the field name. Pins the
/// "fail fast with a pointable error" contract from the S14-0 brief.
#[test]
fn yaml_replay_page_size_zero_is_rejected() {
    let mut f = tempfile::Builder::new()
        .suffix(".yaml")
        .tempfile()
        .expect("tmpfile");
    write!(f, "{YAML_PREFIX}agent:\n  hotl:\n    replay_page_size: 0\n")
        .expect("write tmp yaml");

    let err = Settings::load_from_file(f.path())
        .expect_err("replay_page_size: 0 must fail to deserialize");
    let msg = format!("{err}");
    assert!(
        msg.contains("replay_page_size"),
        "error must mention the offending key name; got: {msg}"
    );
}
