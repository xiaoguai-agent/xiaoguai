//! Subcommand implementations.
//!
//! Each module exposes pure functions taking the dependencies they need
//! (typically a repository trait object) so they are unit-testable without
//! involving clap or `assert_cmd`.

use xiaoguai_config::{Settings, DEV_AUDIT_HMAC_KEY};

/// SEC-15: resolve the audit-chain HMAC signing key for CLI paths that append
/// rows directly (`xiaoguai code`, `xiaoguai schedule`).
///
/// Mirrors the server's fail-closed contract (`xiaoguai-core::run_serve`):
/// prefer the key from the configured env var; fall back to a config-provided
/// `hmac_key` ONLY when the operator overrode it to a non-dev value. The
/// well-known [`DEV_AUDIT_HMAC_KEY`] is never used to sign — a chain signed
/// with a published key is forgeable, so we refuse with actionable guidance
/// instead of silently producing un-trustworthy audit rows.
///
/// # Errors
/// Returns an error when no real signing key is available.
pub fn resolve_audit_signing_key(settings: &Settings) -> anyhow::Result<String> {
    if let Ok(key) = std::env::var(&settings.audit.signing_key_env) {
        if !key.is_empty() {
            return Ok(key);
        }
    }
    if !settings.audit.hmac_key.is_empty() && settings.audit.hmac_key != DEV_AUDIT_HMAC_KEY {
        tracing::warn!(
            "audit chain signing with config-provided `audit.hmac_key` (env \
             {} unset) — prefer the env var for production",
            settings.audit.signing_key_env
        );
        return Ok(settings.audit.hmac_key.clone());
    }
    anyhow::bail!(
        "refusing to sign the audit chain with the well-known dev key: set a real key \
         in env `{}` (or override `audit.hmac_key` in config). A chain signed with the \
         published dev key can be forged and would not verify against the server's chain.",
        settings.audit.signing_key_env
    )
}

pub mod anomaly;
pub mod audit_bundle;
pub mod audit_export;
pub mod backup;
pub mod chat;
pub mod cli_config;
pub mod code;
pub mod completions;
pub mod demo_seed;
pub mod doctor;
pub mod eval;
pub mod hotl;
pub mod init;
pub mod r#loop;
pub mod manpages;
pub mod mcp;
pub mod memory;
pub mod outcomes;
pub mod pack;
pub mod provider;
pub mod remote;
pub mod repl;
pub mod schedule;
pub mod self_update;
pub mod service;
pub mod skills;
pub mod stats;
pub mod style;
pub mod tasks;
pub mod think_filter;
pub mod watch;
