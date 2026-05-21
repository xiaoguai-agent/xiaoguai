//! Casbin RBAC with tenant-scoped roles.
//!
//! The default policy is bundled (compile-time `include_str!`) and streamed
//! into Casbin's in-memory `MemoryAdapter` row by row, so production deploys
//! do not need any filesystem state for the baseline policy and runtime
//! grants such as [`Authz::grant_role`] still work. Custom policies can
//! still be loaded from disk via [`Authz::from_files`].

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
