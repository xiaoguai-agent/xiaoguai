//! Sprint-8 S8-7 (DEC-023.3): wire the `xiaoguai-tasks::skill_author`
//! collaborators against production-grade implementations.
//!
//! This module composes four `Arc<dyn …>` instances:
//!
//! | Trait | Production impl |
//! |---|---|
//! | `SkillProposalRepository` | `xiaoguai_tasks::skill_author_pg::PgSkillProposalRepository` |
//! | `TenantSettingsReader`    | `xiaoguai_tasks::skill_author_pg::PgTenantSettings` |
//! | `SkillAuthorGate`         | [`EnforcerGateAdapter`] over `xiaoguai-api::HotlEnforcer` |
//! | `SkillAuditSink`          | [`AuditSinkAdapter`] over `xiaoguai-audit::PgAuditSink` |
//!
//! The two adapters live here (not in `xiaoguai-tasks`) so the tasks
//! crate doesn't need to depend on `xiaoguai-api` or `xiaoguai-audit`
//! — the trait seams in `skill_author` exist precisely to avoid that
//! coupling.
//!
//! See `skill_author.rs` for the full propose-vs-approve flow and the
//! three audit-row contract (`skill.propose`, `skill.hotl_gate`,
//! `skill.approve`).

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;
use xiaoguai_api::hotl::enforcer::{HotlEnforcer, HotlVerdict};
use xiaoguai_audit::chain::sink::PgAuditSink;
use xiaoguai_audit::AuditEntry;
use xiaoguai_tasks::skill_author::{
    SkillAuditSink, SkillAuthorError, SkillAuthorGate, SkillProposalRepository,
    TenantSettingsReader,
};
use xiaoguai_tasks::skill_author_pg::{PgSkillProposalRepository, PgTenantSettings};

// ---------------------------------------------------------------------------
// Adapters
// ---------------------------------------------------------------------------

/// Adapter mapping a `HotlEnforcer` onto the `SkillAuthorGate` interface.
///
/// `check(tenant, scope)` calls `enforcer.check(tenant, scope, 1.0)`:
///   * `Ok(Allow)` → `Ok(())`
///   * `Ok(Escalate(reason))` → `Err(reason)` (fail-closed for skill
///     authoring; admins explicitly approve, escalation is implicit)
///   * `Ok(Deny(reason))` → `Err(reason)`
///   * `Err(_)` → `Err(format!(…))` (fail-closed on infra failure)
///
/// The `1.0` increment counts the proposal as a single unit of the daily
/// budget (default 5 proposals/tenant/day per `skill_author` scope).
pub struct EnforcerGateAdapter {
    enforcer: Arc<dyn HotlEnforcer>,
}

impl EnforcerGateAdapter {
    #[must_use]
    pub fn new(enforcer: Arc<dyn HotlEnforcer>) -> Self {
        Self { enforcer }
    }

    #[must_use]
    pub fn arc(enforcer: Arc<dyn HotlEnforcer>) -> Arc<dyn SkillAuthorGate> {
        Arc::new(Self::new(enforcer))
    }
}

#[async_trait]
impl SkillAuthorGate for EnforcerGateAdapter {
    async fn check(&self, tenant_id: Uuid, scope: &str) -> Result<(), String> {
        match self.enforcer.check(tenant_id, scope, 1.0).await {
            Ok(HotlVerdict::Allow) => Ok(()),
            Ok(HotlVerdict::Escalate(reason)) => Err(reason),
            Ok(HotlVerdict::Deny(reason)) => Err(reason),
            Err(e) => Err(format!("hotl infra: {e}")),
        }
    }
}

/// Adapter mapping a `PgAuditSink` onto the `SkillAuditSink` interface.
pub struct AuditSinkAdapter {
    sink: Arc<PgAuditSink>,
}

impl AuditSinkAdapter {
    #[must_use]
    pub fn new(sink: Arc<PgAuditSink>) -> Self {
        Self { sink }
    }

    #[must_use]
    pub fn arc(sink: Arc<PgAuditSink>) -> Arc<dyn SkillAuditSink> {
        Arc::new(Self::new(sink))
    }
}

#[async_trait]
impl SkillAuditSink for AuditSinkAdapter {
    async fn record(&self, entry: AuditEntry) -> Result<(), SkillAuthorError> {
        self.sink
            .append(entry)
            .await
            .map(|_| ())
            .map_err(|e| SkillAuthorError::Backend(format!("audit append: {e}")))
    }
}

// ---------------------------------------------------------------------------
// One-shot builder
// ---------------------------------------------------------------------------

/// The four `Arc<dyn …>` instances that compose the skill-author flow.
pub struct SkillAuthorWiring {
    pub proposals: Arc<dyn SkillProposalRepository>,
    pub settings: Arc<dyn TenantSettingsReader>,
    pub gate: Arc<dyn SkillAuthorGate>,
    pub audit: Arc<dyn SkillAuditSink>,
}

/// Compose the production wiring from a Postgres pool + an already-built
/// `HotL` enforcer + an already-built audit sink. Called once at boot.
#[must_use]
pub fn build_skill_author_wiring(
    pool: sqlx::PgPool,
    hotl_enforcer: Arc<dyn HotlEnforcer>,
    audit_sink: Arc<PgAuditSink>,
) -> SkillAuthorWiring {
    SkillAuthorWiring {
        proposals: PgSkillProposalRepository::arc(pool.clone()),
        settings: PgTenantSettings::arc(pool),
        gate: EnforcerGateAdapter::arc(hotl_enforcer),
        audit: AuditSinkAdapter::arc(audit_sink),
    }
}

// ---------------------------------------------------------------------------
// Unit tests — gate + audit adapters use in-memory backing
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;
    use xiaoguai_api::hotl::enforcer::{HotlEnforcerError, HotlVerdictResult};

    /// Bare-bones enforcer that returns a canned verdict — used for unit
    /// coverage of `EnforcerGateAdapter`.
    struct CannedEnforcer {
        verdict: HotlVerdictResult,
    }

    impl CannedEnforcer {
        fn ok(v: HotlVerdict) -> Self {
            Self { verdict: Ok(v) }
        }

        fn err() -> Self {
            Self {
                verdict: Err(HotlEnforcerError::PolicyStore(
                    xiaoguai_api::hotl::HotlPolicyStoreError::Backend("stub".into()),
                )),
            }
        }
    }

    #[async_trait]
    impl HotlEnforcer for CannedEnforcer {
        async fn check(&self, _tenant_id: Uuid, _scope: &str, _amount: f64) -> HotlVerdictResult {
            match &self.verdict {
                Ok(v) => Ok(v.clone()),
                Err(e) => Err(match e {
                    HotlEnforcerError::PolicyStore(_) => HotlEnforcerError::PolicyStore(
                        xiaoguai_api::hotl::HotlPolicyStoreError::Backend("stub".into()),
                    ),
                }),
            }
        }
    }

    #[tokio::test]
    async fn enforcer_gate_allow_passes_through() {
        let enforcer = Arc::new(CannedEnforcer::ok(HotlVerdict::Allow));
        let gate = EnforcerGateAdapter::new(enforcer);
        assert!(gate.check(Uuid::new_v4(), "skill_author").await.is_ok());
    }

    #[tokio::test]
    async fn enforcer_gate_deny_propagates_reason() {
        let enforcer = Arc::new(CannedEnforcer::ok(HotlVerdict::Deny(
            "daily cap reached".into(),
        )));
        let gate = EnforcerGateAdapter::new(enforcer);
        let err = gate
            .check(Uuid::new_v4(), "skill_author")
            .await
            .unwrap_err();
        assert_eq!(err, "daily cap reached");
    }

    #[tokio::test]
    async fn enforcer_gate_escalate_treated_as_deny() {
        // Skill authoring intentionally treats escalation as deny so the
        // admin approval flow is the only path through.
        let enforcer = Arc::new(CannedEnforcer::ok(HotlVerdict::Escalate(
            "review by admin".into(),
        )));
        let gate = EnforcerGateAdapter::new(enforcer);
        assert!(gate.check(Uuid::new_v4(), "skill_author").await.is_err());
    }

    #[tokio::test]
    async fn enforcer_gate_fail_closed_on_infra_error() {
        let enforcer = Arc::new(CannedEnforcer::err());
        let gate = EnforcerGateAdapter::new(enforcer);
        let err = gate
            .check(Uuid::new_v4(), "skill_author")
            .await
            .unwrap_err();
        assert!(err.contains("hotl infra"), "got: {err}");
    }

    // Construct an audit entry; we only need its shape — the adapter
    // delegates to PgAuditSink::append which is exercised by the audit
    // crate's own integration tests.
    fn sample_entry() -> AuditEntry {
        AuditEntry {
            ts: Utc::now(),
            tenant_id: "t-1".into(),
            actor: "agent:test".into(),
            action: "skill.propose".into(),
            resource: Some("skill:demo:0.1.0".into()),
            details: json!({"name": "demo"}),
        }
    }

    // We don't construct a real PG pool here — the adapter is just a
    // thin wrapper. Confirm the type composes; functional coverage lives
    // in `xiaoguai-audit/src/sink.rs`.
    #[test]
    fn audit_sink_adapter_composes() {
        let _ = sample_entry();
    }
}
