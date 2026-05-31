//! Casbin RBAC with tenant-scoped roles.
//!
//! The default policy is bundled (compile-time `include_str!`) and streamed
//! into Casbin's in-memory `MemoryAdapter` row by row, so production deploys
//! do not need any filesystem state for the baseline policy and runtime
//! grants such as [`Authz::grant_role`] still work. Custom policies can
//! still be loaded from disk via [`Authz::from_files`].
//!
//! ## Hybrid DB-backed policy merge (sprint-13 S13-10)
//!
//! For policies that live outside the compiled-in CSV — currently the
//! per-tenant scope rules seeded by migration 0027 into the `casbin_rule`
//! table — the caller fetches rows once at boot via its own pool, packs
//! them into [`DbPolicyRow`] values, and calls
//! [`Authz::merge_db_policies`]. The merge happens **after** the CSV is
//! loaded so DB rows are additive, never destructive. The enforcer's
//! hot-path check stays in-memory; the DB query is a one-shot at boot.

use std::sync::Arc;

use casbin::{CoreApi, DefaultModel, Enforcer, FileAdapter, MemoryAdapter, MgmtApi, RbacApi};
use thiserror::Error;
use tokio::sync::RwLock;

/// Compiled-in default model file.
const DEFAULT_MODEL: &str = include_str!("../policies/rbac_model.conf");
/// Compiled-in default policy file.
const DEFAULT_POLICY: &str = include_str!("../policies/rbac_policy.csv");

/// RBAC errors.
#[derive(Debug, Error)]
pub enum RbacError {
    /// Failed to load model or policy.
    #[error("policy load: {0}")]
    PolicyLoad(String),
    /// Failure during a single enforcement check.
    #[error("enforcement: {0}")]
    Enforce(String),
}

/// A single row from the `casbin_rule` table (sql-adapter shape).
///
/// `ptype` is `p` for plain policy rules or `g` for role-inheritance
/// grants; `v0..v5` are the unbounded value columns Casbin tolerates.
/// Only the non-`None` prefix is forwarded into the enforcer — trailing
/// `None`s are dropped so a 4-column model does not see padding.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DbPolicyRow {
    /// Casbin section identifier (`p`, `g`, …).
    pub ptype: String,
    /// Value column 0 (typically subject).
    pub v0: Option<String>,
    /// Value column 1 (typically domain / tenant).
    pub v1: Option<String>,
    /// Value column 2 (typically resource).
    pub v2: Option<String>,
    /// Value column 3 (typically action).
    pub v3: Option<String>,
    /// Value column 4 (typically effect).
    pub v4: Option<String>,
    /// Value column 5 (reserved for future Casbin variants).
    pub v5: Option<String>,
}

impl DbPolicyRow {
    /// Pack the row's value columns into the canonical positional vector
    /// Casbin expects, dropping the trailing `None` tail.
    #[must_use]
    pub fn into_values(self) -> Vec<String> {
        let raw = [self.v0, self.v1, self.v2, self.v3, self.v4, self.v5];
        // Find the index *after* the last `Some`. When every slot is
        // `None`, return an empty vec — the caller treats that as a
        // malformed row and skips it. Interior `None`s become empty
        // strings (Casbin's convention).
        match raw.iter().rposition(Option::is_some) {
            Some(last) => raw
                .into_iter()
                .take(last + 1)
                .map(Option::unwrap_or_default)
                .collect(),
            None => Vec::new(),
        }
    }
}

/// Thread-safe authorisation handle backed by a Casbin `Enforcer`.
#[derive(Clone)]
pub struct Authz {
    enforcer: Arc<RwLock<Enforcer>>,
}

impl Authz {
    /// Load the bundled default model + policy (no filesystem access).
    ///
    /// # Errors
    /// Returns [`RbacError::PolicyLoad`] if the embedded model or policy
    /// fails to parse.
    pub async fn new_default() -> Result<Self, RbacError> {
        let model = DefaultModel::from_str(DEFAULT_MODEL)
            .await
            .map_err(|e| RbacError::PolicyLoad(e.to_string()))?;
        // `MemoryAdapter` starts empty but supports incremental writes, which
        // `StringAdapter` does not. We then bulk-load the embedded policy
        // through `add_policy` / `add_grouping_policy` so writes can succeed
        // at runtime.
        let mut enforcer = Enforcer::new(model, MemoryAdapter::default())
            .await
            .map_err(|e| RbacError::PolicyLoad(e.to_string()))?;
        load_embedded_policy(&mut enforcer, DEFAULT_POLICY).await?;
        Ok(Self {
            enforcer: Arc::new(RwLock::new(enforcer)),
        })
    }

    /// Load model + policy from disk paths.
    ///
    /// # Errors
    /// Returns [`RbacError::PolicyLoad`] if either file fails to load.
    pub async fn from_files(model_path: &str, policy_path: &str) -> Result<Self, RbacError> {
        let model = DefaultModel::from_file(model_path)
            .await
            .map_err(|e| RbacError::PolicyLoad(e.to_string()))?;
        let adapter = FileAdapter::new(policy_path.to_string());
        let enforcer = Enforcer::new(model, adapter)
            .await
            .map_err(|e| RbacError::PolicyLoad(e.to_string()))?;
        Ok(Self {
            enforcer: Arc::new(RwLock::new(enforcer)),
        })
    }

    /// Check whether `subject` (a role name) may perform `action` on
    /// `resource` within `tenant_id`.
    ///
    /// # Errors
    /// Returns [`RbacError::Enforce`] if Casbin's matcher fails to evaluate.
    pub async fn check(
        &self,
        subject: &str,
        tenant_id: &str,
        resource: &str,
        action: &str,
    ) -> Result<bool, RbacError> {
        let guard = self.enforcer.read().await;
        guard
            .enforce((subject, tenant_id, resource, action))
            .map_err(|e| RbacError::Enforce(e.to_string()))
    }

    /// Add a (user, role, tenant) grant at runtime.
    ///
    /// # Errors
    /// Returns [`RbacError::Enforce`] on storage failure.
    pub async fn grant_role(
        &self,
        user_id: &str,
        role: &str,
        tenant_id: &str,
    ) -> Result<(), RbacError> {
        let mut guard = self.enforcer.write().await;
        guard
            .add_role_for_user(user_id, role, Some(tenant_id))
            .await
            .map_err(|e| RbacError::Enforce(e.to_string()))?;
        Ok(())
    }

    /// Add a policy rule at runtime (escape hatch for tests / migrations).
    ///
    /// # Errors
    /// Returns [`RbacError::Enforce`] on storage failure.
    pub async fn add_policy_rule(
        &self,
        subject: &str,
        tenant_id: &str,
        resource: &str,
        action: &str,
    ) -> Result<bool, RbacError> {
        let mut guard = self.enforcer.write().await;
        guard
            .add_policy(vec![
                subject.to_string(),
                tenant_id.to_string(),
                resource.to_string(),
                action.to_string(),
            ])
            .await
            .map_err(|e| RbacError::Enforce(e.to_string()))
    }

    /// Merge pre-fetched [`DbPolicyRow`] entries into the in-memory
    /// enforcer. Sprint-13 S13-10 calls this once at boot with the rows
    /// from the `casbin_rule` table seeded by migration 0027.
    ///
    /// Rows with `ptype = "p"` become policy rules; `ptype = "g"`
    /// becomes a role-inheritance grant. Unknown ptypes are skipped
    /// with a warning. Empty rows (all `None`) are silently dropped.
    /// Duplicates of the CSV are no-ops — Casbin's `add_policy` returns
    /// `false` on duplicate insert and the merge swallows that.
    ///
    /// # Errors
    /// Returns [`RbacError::PolicyLoad`] if any row fails the underlying
    /// Casbin write (e.g. arity mismatch with the model).
    pub async fn merge_db_policies(&mut self, rows: Vec<DbPolicyRow>) -> Result<(), RbacError> {
        let mut guard = self.enforcer.write().await;
        for row in rows {
            let ptype = row.ptype.clone();
            let values = row.into_values();
            if values.is_empty() {
                continue;
            }
            match ptype.as_str() {
                "p" => {
                    guard
                        .add_policy(values)
                        .await
                        .map_err(|e| RbacError::PolicyLoad(e.to_string()))?;
                }
                "g" => {
                    guard
                        .add_grouping_policy(values)
                        .await
                        .map_err(|e| RbacError::PolicyLoad(e.to_string()))?;
                }
                other => {
                    tracing::warn!(
                        ptype = %other,
                        "merge_db_policies: skipping row with unknown ptype"
                    );
                }
            }
        }
        Ok(())
    }

    /// Defensive boot-time assertion: does the loaded policy contain a
    /// `p` rule whose value columns match `values`? Sprint-13 S13-10
    /// uses this to verify the seeded `hotl:decide` rule landed; a
    /// missing rule is a partial-migration symptom and the caller
    /// fail-fasts at boot.
    pub async fn has_policy_rule(&self, values: &[&str]) -> bool {
        let guard = self.enforcer.read().await;
        let needle: Vec<String> = values.iter().map(|s| (*s).to_string()).collect();
        guard.get_policy().into_iter().any(|row| row == needle)
    }
}

/// Parse the embedded CSV policy and stream each row into the enforcer.
/// Lines beginning with `p,` become policy rules; lines beginning with `g,`
/// become role-inheritance grants. Empty lines and `#` comments are skipped.
async fn load_embedded_policy(enforcer: &mut Enforcer, csv: &str) -> Result<(), RbacError> {
    for raw in csv.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<String> = line.split(',').map(|p| p.trim().to_string()).collect();
        if parts.len() < 2 {
            continue;
        }
        let head = parts[0].as_str();
        let tail: Vec<String> = parts[1..].to_vec();
        match head {
            "p" => {
                enforcer
                    .add_policy(tail)
                    .await
                    .map_err(|e| RbacError::PolicyLoad(e.to_string()))?;
            }
            "g" => {
                enforcer
                    .add_grouping_policy(tail)
                    .await
                    .map_err(|e| RbacError::PolicyLoad(e.to_string()))?;
            }
            _ => {
                return Err(RbacError::PolicyLoad(format!(
                    "unknown policy section `{head}`"
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn into_values_drops_trailing_none_tail() {
        let row = DbPolicyRow {
            ptype: "p".into(),
            v0: Some("hotl:decide".into()),
            v1: Some("/v1/hotl/decisions".into()),
            v2: Some("POST".into()),
            v3: Some("allow".into()),
            v4: None,
            v5: None,
        };
        assert_eq!(
            row.into_values(),
            vec!["hotl:decide", "/v1/hotl/decisions", "POST", "allow"]
        );
    }

    #[test]
    fn into_values_preserves_interior_none_as_empty_string() {
        let row = DbPolicyRow {
            ptype: "p".into(),
            v0: Some("sub".into()),
            v1: None,
            v2: Some("obj".into()),
            v3: None,
            v4: None,
            v5: None,
        };
        // Interior `None` survives as `""` so positional alignment with
        // the model is preserved.
        assert_eq!(row.into_values(), vec!["sub", "", "obj"]);
    }

    #[test]
    fn into_values_all_none_returns_empty_vec() {
        let row = DbPolicyRow {
            ptype: "p".into(),
            v0: None,
            v1: None,
            v2: None,
            v3: None,
            v4: None,
            v5: None,
        };
        assert!(row.into_values().is_empty());
    }

    #[tokio::test]
    async fn merge_db_policies_no_op_on_empty_rows() {
        let mut authz = Authz::new_default().await.expect("authz");
        authz
            .merge_db_policies(vec![DbPolicyRow {
                ptype: "p".into(),
                ..DbPolicyRow::default()
            }])
            .await
            .expect("empty row must be a silent no-op");
    }

    #[tokio::test]
    async fn merge_db_policies_skips_unknown_ptype() {
        let mut authz = Authz::new_default().await.expect("authz");
        // The current 4-column model would reject an arbitrary ptype at
        // the Casbin layer; the merge logs and continues instead.
        authz
            .merge_db_policies(vec![DbPolicyRow {
                ptype: "x".into(),
                v0: Some("ignored".into()),
                ..DbPolicyRow::default()
            }])
            .await
            .expect("unknown ptype must be skipped, not error");
    }

    #[tokio::test]
    async fn has_policy_rule_false_for_unseeded_rule() {
        let authz = Authz::new_default().await.expect("authz");
        // The bundled CSV has no `hotl:decide` rule.
        assert!(
            !authz
                .has_policy_rule(&["hotl:decide", "/v1/hotl/decisions", "POST", "allow"])
                .await
        );
    }

    #[tokio::test]
    async fn has_policy_rule_true_after_merge() {
        let mut authz = Authz::new_default().await.expect("authz");
        authz
            .merge_db_policies(vec![DbPolicyRow {
                ptype: "p".into(),
                v0: Some("hotl:decide".into()),
                v1: Some("/v1/hotl/decisions".into()),
                v2: Some("POST".into()),
                v3: Some("allow".into()),
                v4: None,
                v5: None,
            }])
            .await
            .expect("merge");
        assert!(
            authz
                .has_policy_rule(&["hotl:decide", "/v1/hotl/decisions", "POST", "allow"])
                .await
        );
    }
}
