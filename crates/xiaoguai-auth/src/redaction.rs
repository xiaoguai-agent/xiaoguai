//! sprint-13 S13-4: `RedactionRules` — `JSONPath`-driven `HotL` argument masking.
//!
//! This module is the **read-side** counterpart to `HotlRedactionRepo` in
//! `xiaoguai-storage`. The repo loads `RedactionPolicyRow`s for a tenant; this
//! module wraps that vector in a value object that knows how to:
//!
//! 1. Pick the right rule for a given scope — exact-scope wins over `*`
//!    catch-all (the storage layer already sorts in this order, see
//!    `hotl_redaction.rs` ORDER BY clause).
//! 2. Walk the JSON tree and replace matched nodes with `"***"`, returning a
//!    fresh `serde_json::Value` so the original `args` is never mutated
//!    (project coding-style rule: immutability).
//! 3. Warn **once per `RedactionRules` instance** when the rule set is empty
//!    — covers the "no policy configured for tenant" path so operators
//!    notice unredacted SSE traffic without spamming logs for every tool call.
//!
//! ## Scope of this module (sprint boundary)
//!
//! - **Only the SSE-emission path** is handled. `applies_to` filter checks
//!   for `"sse"`; rules tagged audit-only are skipped here. Audit-side
//!   application (`apply_with_audit_side`) lands in sprint-14 alongside the
//!   admin CRUD surface.
//! - **No wiring into `SuspendingHotlGate`** — that is S13-6's job. This
//!   module only exposes the API.
//!
//! ## Cross-refs
//!
//! - DEC-HLD-014 (`HotL` redaction policy table)
//! - `guardrails.md` §3.1 (mandatory redaction on the SSE path)
//! - `lld-agent.md` §4.6 (`RedactionRules` construction lifetime)

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;
use xiaoguai_storage::repositories::{
    error::RepoError,
    hotl_redaction::{HotlRedactionRepo, RedactionPolicyRow},
};

/// `applies_to` tag for the SSE emission path. The audit-side tag is
/// `"audit"` and is handled in sprint-14.
const APPLIES_TO_SSE: &str = "sse";

/// `*` is the wildcard scope marker (matches any caller scope).
const WILDCARD_SCOPE: &str = "*";

/// The string we substitute in place of any matched JSON leaf.
const REDACTED_PLACEHOLDER: &str = "***";

/// Errors raised by `xiaoguai-auth::redaction`.
///
/// Today this only wraps `RepoError` from the load path; once admin CRUD
/// arrives in sprint-14 we may grow validation variants here.
#[derive(Debug, Error)]
pub enum AuthError {
    /// Failed to load redaction policies from storage.
    #[error("failed to load redaction policies: {0}")]
    Storage(#[from] RepoError),
}

/// Holds a loaded rule set for one tenant and applies it to tool-call args.
///
/// Cheap to clone? No — the rule vector is moved in. Construct once at
/// request-context build time and pass by reference to the gate (`&Self`).
#[derive(Debug)]
pub struct RedactionRules {
    rules: Vec<RedactionPolicyRow>,
    /// Set to `true` the first time `apply()` is called on an empty rule
    /// set. Subsequent empty-rule applies skip the `warn!`. Atomic because
    /// `apply()` takes `&self` (the gate holds an `Arc<RedactionRules>` and
    /// concurrent requests share it).
    warned_empty: AtomicBool,
    /// Test-only observable: count of warn emissions. The production code
    /// path checks `warned_empty` to decide whether to emit; this counter
    /// just lets unit tests assert "warn was called exactly once".
    /// Hidden from the public API surface — only `cfg(test)` reads it.
    warn_count: AtomicU64,
}

impl RedactionRules {
    /// Construct from a pre-loaded rule vector. Used by `from_storage` and
    /// by unit tests that bypass the repo trait.
    #[must_use]
    pub fn new(rules: Vec<RedactionPolicyRow>) -> Self {
        Self {
            rules,
            warned_empty: AtomicBool::new(false),
            warn_count: AtomicU64::new(0),
        }
    }

    /// Load all rules for `tenant_id` from storage.
    ///
    /// The repo already sorts exact-scope before `*`, so `self.rules` is
    /// in the order `apply()` wants to iterate.
    pub async fn from_storage(
        repo: &dyn HotlRedactionRepo,
        tenant_id: Uuid,
    ) -> Result<Self, AuthError> {
        let rows = repo.load_for_tenant(tenant_id).await?;
        Ok(Self::new(rows))
    }

    /// Apply the first matching rule to `args` and return a fresh `Value`.
    ///
    /// Semantics:
    /// - If `self.rules` is empty: emit a warn-once log, return `args.clone()`.
    /// - Otherwise: walk `self.rules` in order; pick the first whose
    ///   `scope == requested_scope || scope == "*"` AND whose `applies_to`
    ///   contains `"sse"`.
    /// - If no rule matches: return `args.clone()` (no warn — empty-rule-set
    ///   is the loud case; "no rule for this scope" is normal).
    /// - If a rule matches: evaluate its `JSONPath` against `args` and replace
    ///   every matched leaf with the string `"***"`.
    ///
    /// `JSONPath` evaluation uses `jsonpath_lib`; an unparseable selector
    /// is treated as a no-match (we log an error so it surfaces in alerts,
    /// but we do not panic at request time).
    #[must_use]
    pub fn apply(&self, scope: &str, args: &Value) -> Value {
        if self.rules.is_empty() {
            self.warn_empty_once();
            return args.clone();
        }

        let Some(rule) = self.pick_matching_rule(scope) else {
            return args.clone();
        };

        match jsonpath_lib::replace_with(args.clone(), &rule.jsonpath, &mut |_| {
            Some(Value::String(REDACTED_PLACEHOLDER.into()))
        }) {
            Ok(masked) => masked,
            Err(err) => {
                tracing::error!(
                    target: "xiaoguai_auth::redaction",
                    policy_id = %rule.id,
                    jsonpath = %rule.jsonpath,
                    error = %err,
                    "JSONPath evaluation failed; emitting args verbatim (no mask)",
                );
                args.clone()
            }
        }
    }

    /// Return the id of the first rule that would match `scope` on the SSE
    /// path. S13-6 calls this so the audit row's `redaction_policy_id` FK
    /// points to the same rule that actually masked the args.
    ///
    /// Returns `None` if no rule matches (including the empty-rule-set case).
    #[must_use]
    pub fn matching_rule_id(&self, scope: &str) -> Option<Uuid> {
        self.pick_matching_rule(scope).map(|r| r.id)
    }

    /// Test-only observable: how many warn-once emissions have fired on
    /// this instance. In production the only consumer is the integration
    /// test in S13-6.
    #[doc(hidden)]
    #[must_use]
    pub fn warn_count(&self) -> u64 {
        self.warn_count.load(Ordering::Relaxed)
    }

    fn pick_matching_rule(&self, scope: &str) -> Option<&RedactionPolicyRow> {
        self.rules.iter().find(|r| {
            (r.scope == scope || r.scope == WILDCARD_SCOPE)
                && r.applies_to.iter().any(|t| t == APPLIES_TO_SSE)
        })
    }

    fn warn_empty_once(&self) {
        // `compare_exchange` returns Ok only for the first thread that
        // flips false -> true. Subsequent threads see the existing `true`
        // and skip the warn — exactly the "once per instance" semantic.
        if self
            .warned_empty
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            tracing::warn!(
                target: "xiaoguai_auth::redaction",
                "no HotL redaction policy configured — tool-call args emitted verbatim on SSE",
            );
            self.warn_count.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use chrono::Utc;
    use serde_json::json;

    use super::*;

    fn rule(scope: &str, jsonpath: &str, applies_to: &[&str]) -> RedactionPolicyRow {
        RedactionPolicyRow {
            id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            scope: scope.into(),
            jsonpath: jsonpath.into(),
            applies_to: applies_to.iter().map(|s| (*s).into()).collect(),
            created_at: Utc::now(),
            // Sprint-14 S14-2: revision-tracking fields. Tests construct
            // synthetic active rules, so `active = true` + no prior.
            active: true,
            created_by: "test".to_string(),
            supersedes_policy_id: None,
        }
    }

    #[test]
    fn apply_replaces_top_level_field() {
        let rules = RedactionRules::new(vec![rule("scope_a", "$.password", &["sse"])]);
        let args = json!({ "password": "x" });
        let out = rules.apply("scope_a", &args);
        assert_eq!(out, json!({ "password": "***" }));
    }

    #[test]
    fn apply_replaces_nested_field() {
        let rules = RedactionRules::new(vec![rule("scope_a", "$.headers.authorization", &["sse"])]);
        let args = json!({ "headers": { "authorization": "Bearer x", "x-trace": "abc" } });
        let out = rules.apply("scope_a", &args);
        assert_eq!(
            out,
            json!({ "headers": { "authorization": "***", "x-trace": "abc" } })
        );
    }

    #[test]
    fn apply_handles_array_selector() {
        let rules = RedactionRules::new(vec![rule("scope_a", "$.users[*].api_key", &["sse"])]);
        let args = json!({
            "users": [
                { "name": "a", "api_key": "x" },
                { "name": "b", "api_key": "y" }
            ]
        });
        let out = rules.apply("scope_a", &args);
        assert_eq!(
            out,
            json!({
                "users": [
                    { "name": "a", "api_key": "***" },
                    { "name": "b", "api_key": "***" }
                ]
            })
        );
    }

    #[test]
    fn apply_no_match_returns_clone() {
        let rules = RedactionRules::new(vec![rule("scope_a", "$.password", &["sse"])]);
        let args = json!({ "other": "x" });
        let out = rules.apply("scope_a", &args);
        assert_eq!(out, args);
    }

    #[test]
    fn apply_empty_ruleset_returns_clone_and_warns_once() {
        let rules = RedactionRules::new(vec![]);
        let args = json!({ "password": "x" });

        let out1 = rules.apply("scope_a", &args);
        assert_eq!(out1, args, "empty rule set must return args unchanged");
        assert_eq!(rules.warn_count(), 1, "first call must warn");

        let out2 = rules.apply("scope_a", &args);
        assert_eq!(out2, args);
        assert_eq!(
            rules.warn_count(),
            1,
            "second call must NOT re-warn (warn-once semantic)"
        );
    }

    #[test]
    fn apply_picks_exact_scope_over_wildcard() {
        // Repo would sort these exact-first, then wildcard. We feed them in
        // that order to mirror production layout.
        let rules = RedactionRules::new(vec![
            rule("scope_a", "$.y", &["sse"]),
            rule("*", "$.x", &["sse"]),
        ]);
        let args = json!({ "x": "a", "y": "b" });
        let out = rules.apply("scope_a", &args);
        assert_eq!(
            out,
            json!({ "x": "a", "y": "***" }),
            "exact-scope rule wins; wildcard ignored when an exact match exists"
        );
    }

    #[test]
    fn apply_skips_rules_not_in_applies_to_sse() {
        // applies_to = ["audit"] only — SSE path must ignore it.
        let rules = RedactionRules::new(vec![rule("scope_a", "$.password", &["audit"])]);
        let args = json!({ "password": "x" });
        let out = rules.apply("scope_a", &args);
        assert_eq!(out, args, "audit-only rule must not fire on the SSE path");
    }

    #[test]
    fn matching_rule_id_returns_first_matching_id() {
        let r1 = rule("scope_a", "$.x", &["sse"]);
        let r2 = rule("scope_a", "$.y", &["sse"]);
        let id1 = r1.id;
        let rules = RedactionRules::new(vec![r1, r2]);
        let got = rules.matching_rule_id("scope_a");
        assert_eq!(got, Some(id1));
    }

    #[test]
    fn matching_rule_id_returns_none_for_no_match() {
        let rules = RedactionRules::new(vec![rule("scope_a", "$.x", &["sse"])]);
        assert_eq!(rules.matching_rule_id("scope_b"), None);
    }

    #[test]
    fn matching_rule_id_returns_none_for_empty_ruleset() {
        let rules = RedactionRules::new(vec![]);
        assert_eq!(rules.matching_rule_id("scope_a"), None);
    }

    #[test]
    fn matching_rule_id_falls_back_to_wildcard() {
        let wildcard = rule("*", "$.x", &["sse"]);
        let id = wildcard.id;
        let rules = RedactionRules::new(vec![wildcard]);
        assert_eq!(rules.matching_rule_id("scope_a"), Some(id));
    }

    #[test]
    fn apply_does_not_mutate_input() {
        let rules = RedactionRules::new(vec![rule("scope_a", "$.password", &["sse"])]);
        let args = json!({ "password": "x" });
        let _ = rules.apply("scope_a", &args);
        assert_eq!(
            args,
            json!({ "password": "x" }),
            "apply must not mutate caller's args (immutability)"
        );
    }

    /// R13-3 perf budget: 5 rules × 20-key nested args under 100µs (p99).
    /// We average 1000 iterations — `std::time::Instant`, no `criterion`.
    /// If the budget is missed, we print the number for sprint-14 follow-up
    /// but do NOT fail the test (per the brief: "don't block on it").
    #[test]
    #[allow(clippy::cast_precision_loss)] // bench averaging; 1000 iters fits f64 mantissa
    fn microbench_apply_under_100us_p99() {
        const ITERS: u32 = 1000;
        const BUDGET_US: f64 = 100.0;

        let rules = RedactionRules::new(vec![
            rule("scope_a", "$.password", &["sse"]),
            rule("scope_a", "$.token", &["sse"]),
            rule("scope_a", "$.headers.authorization", &["sse"]),
            rule("scope_a", "$.users[*].api_key", &["sse"]),
            rule("scope_a", "$.payload.secret", &["sse"]),
        ]);
        let args = json!({
            "password": "p", "token": "t", "other": "o",
            "headers": { "authorization": "Bearer x", "x-trace": "abc", "x-corr": "z" },
            "users": [
                { "name": "a", "api_key": "ak1", "role": "admin" },
                { "name": "b", "api_key": "ak2", "role": "user" }
            ],
            "payload": { "secret": "s", "public": "p" },
            "meta": { "k1": 1, "k2": 2, "k3": 3, "k4": 4, "k5": 5 },
            "extra": { "x1": "v1", "x2": "v2" }
        });

        // Warmup — JIT-style caches inside jsonpath_lib's parser.
        for _ in 0..50 {
            let _ = rules.apply("scope_a", &args);
        }

        let start = Instant::now();
        for _ in 0..ITERS {
            let _ = rules.apply("scope_a", &args);
        }
        let elapsed = start.elapsed();
        let per_call_us = elapsed.as_micros() as f64 / f64::from(ITERS);

        // Soft assertion: log loudly but don't fail. Sprint-14 picks up if
        // we're over budget on real hardware.
        if per_call_us > BUDGET_US {
            eprintln!(
                "WARN: RedactionRules::apply averaged {per_call_us:.2} µs/call \
                 over {ITERS} iters — over the {BUDGET_US}µs budget. \
                 Document in PR body for sprint-14 follow-up."
            );
        } else {
            eprintln!(
                "OK: RedactionRules::apply averaged {per_call_us:.2} µs/call \
                 over {ITERS} iters (budget = {BUDGET_US}µs)."
            );
        }
    }
}
