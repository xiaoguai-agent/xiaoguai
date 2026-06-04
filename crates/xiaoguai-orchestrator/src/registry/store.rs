//! Persistence abstraction for the agent registry.
//!
//! # Design
//!
//! The `RegistryStore` trait decouples the in-memory `AgentRegistry` from any
//! backing store.  The crate ships `InMemoryStore` as the production-ready
//! default.  `SqliteStore` is stubbed with `TODO` markers so implementors have a
//! clear integration path.
//!
//! ## Why not embed persistence in `AgentRegistry`?
//!
//! `AgentRegistry` owns the hot-path lookup; `RegistryStore` owns the
//! cold-path snapshot / reload cycle.  Keeping them separate means the
//! registry never pays a DB round-trip on every `lookup_by_capability` call.
//!
//! ## Usage pattern
//!
//! ```text
//! startup:
//!   store.load_all() → Vec<AgentSpec>
//!   → re-hydrate registry (callers re-register agents with live impls)
//!
//! on register/unregister:
//!   store.save(spec)
//!   store.remove(name)
//! ```
//!
//! # Deferred
//! - `SqliteStore` full implementation (v1.3) — blocked on deciding the PG schema
//!   for capability arrays and cost columns.
//! - Distributed gossip / watch channel for multi-node registry sync (v1.4).

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::OrchestratorError;

use super::AgentSpec;

// ── RegistryStore trait ───────────────────────────────────────────────────────

/// Persistence interface for `AgentSpec` records.
///
/// All methods are `async` so implementors may perform I/O without blocking.
#[async_trait]
pub trait RegistryStore: Send + Sync {
    /// Persist (insert or update) an `AgentSpec`.
    async fn save(&self, spec: &AgentSpec) -> Result<(), OrchestratorError>;

    /// Remove the spec with the given name.  Returns `Ok(())` whether or not
    /// the record existed (idempotent).
    async fn remove(&self, name: &str) -> Result<(), OrchestratorError>;

    /// Load all persisted specs.
    async fn load_all(&self) -> Result<Vec<AgentSpec>, OrchestratorError>;
}

// ── InMemoryStore ─────────────────────────────────────────────────────────────

/// A `RegistryStore` that keeps all data in a `HashMap` behind a `Mutex`.
///
/// This is the default implementation used in tests, single-process
/// deployments, and anywhere persistence across restarts is not required.
#[derive(Default)]
pub struct InMemoryStore {
    map: Mutex<HashMap<String, AgentSpec>>,
}

impl InMemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl RegistryStore for InMemoryStore {
    async fn save(&self, spec: &AgentSpec) -> Result<(), OrchestratorError> {
        let mut map = self.map.lock().unwrap();
        map.insert(spec.name.clone(), spec.clone());
        Ok(())
    }

    async fn remove(&self, name: &str) -> Result<(), OrchestratorError> {
        let mut map = self.map.lock().unwrap();
        map.remove(name);
        Ok(())
    }

    async fn load_all(&self) -> Result<Vec<AgentSpec>, OrchestratorError> {
        let map = self.map.lock().unwrap();
        let mut specs: Vec<AgentSpec> = map.values().cloned().collect();
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(specs)
    }
}

// ── SqliteStore stub ─────────────────────────────────────────────────────────────

/// `SQLite`-backed registry store.
///
/// **Deferred to v1.3** — the PG schema for capability arrays and cost
/// columns, and the migration file are not yet defined.
///
/// All methods currently return `OrchestratorError::Internal("not implemented")`.
/// The struct is intentionally public so callers can depend on the type and
/// swap it in when the impl lands.
pub struct SqliteStore {
    // TODO(v1.3): `pool: sqlx::SqlitePool` — held here once the migration and
    //   DDL (`agent_registry` table with JSONB `capabilities` column) land.
    _private: (),
}

impl SqliteStore {
    /// Construct a `SqliteStore`.
    ///
    /// TODO(v1.3): accept a `sqlx::SqlitePool` argument.
    #[must_use]
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for SqliteStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RegistryStore for SqliteStore {
    async fn save(&self, _spec: &AgentSpec) -> Result<(), OrchestratorError> {
        // TODO(v1.3): INSERT INTO agent_registry (name, version, capabilities,
        //   cost_hint) VALUES ($1, $2, $3::jsonb, $4)
        //   ON CONFLICT (name) DO UPDATE SET ...
        Err(OrchestratorError::Internal(
            "SqliteStore::save not yet implemented (deferred to v1.3)".to_owned(),
        ))
    }

    async fn remove(&self, _name: &str) -> Result<(), OrchestratorError> {
        // TODO(v1.3): DELETE FROM agent_registry WHERE name = $1
        Err(OrchestratorError::Internal(
            "SqliteStore::remove not yet implemented (deferred to v1.3)".to_owned(),
        ))
    }

    async fn load_all(&self) -> Result<Vec<AgentSpec>, OrchestratorError> {
        // TODO(v1.3): SELECT * FROM agent_registry ORDER BY name
        Err(OrchestratorError::Internal(
            "SqliteStore::load_all not yet implemented (deferred to v1.3)".to_owned(),
        ))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{test_helpers::make_spec, Capability, ResultShape, TaskShape};

    fn spec_a() -> AgentSpec {
        make_spec("agent-a", vec![("billing", "draft_email")], 1.0)
    }

    fn spec_b() -> AgentSpec {
        make_spec("agent-b", vec![("incident", "summarize")], 2.0)
    }

    // ── InMemoryStore ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn save_and_load_all() {
        let store = InMemoryStore::new();
        store.save(&spec_a()).await.unwrap();
        store.save(&spec_b()).await.unwrap();
        let all = store.load_all().await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "agent-a");
        assert_eq!(all[1].name, "agent-b");
    }

    #[tokio::test]
    async fn save_overwrites_existing() {
        let store = InMemoryStore::new();
        store.save(&spec_a()).await.unwrap();
        let mut updated = spec_a();
        updated.cost_hint = 99.0;
        store.save(&updated).await.unwrap();
        let all = store.load_all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert!((all[0].cost_hint - 99.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn remove_existing_reduces_count() {
        let store = InMemoryStore::new();
        store.save(&spec_a()).await.unwrap();
        store.save(&spec_b()).await.unwrap();
        store.remove("agent-a").await.unwrap();
        let all = store.load_all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "agent-b");
    }

    #[tokio::test]
    async fn remove_nonexistent_is_idempotent() {
        let store = InMemoryStore::new();
        // Should not return an error.
        store.remove("ghost").await.unwrap();
    }

    #[tokio::test]
    async fn load_all_empty_returns_empty_vec() {
        let store = InMemoryStore::new();
        let all = store.load_all().await.unwrap();
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn load_all_sorted_by_name() {
        let store = InMemoryStore::new();
        store.save(&spec_b()).await.unwrap();
        store.save(&spec_a()).await.unwrap();
        let all = store.load_all().await.unwrap();
        let names: Vec<_> = all.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["agent-a", "agent-b"]);
    }

    // ── SqliteStore stub ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn pg_store_save_returns_not_implemented() {
        let store = SqliteStore::new();
        let err = store.save(&spec_a()).await.unwrap_err();
        assert!(err.to_string().contains("not yet implemented"));
    }

    #[tokio::test]
    async fn pg_store_load_all_returns_not_implemented() {
        let store = SqliteStore::new();
        let err = store.load_all().await.unwrap_err();
        assert!(err.to_string().contains("not yet implemented"));
    }

    // ── Trait object usage ────────────────────────────────────────────────────

    #[tokio::test]
    async fn trait_object_dispatch_works() {
        let store: Box<dyn RegistryStore> = Box::new(InMemoryStore::new());
        store.save(&spec_a()).await.unwrap();
        let all = store.load_all().await.unwrap();
        assert_eq!(all.len(), 1);
    }

    // ── Spec field round-trip ─────────────────────────────────────────────────

    #[tokio::test]
    async fn spec_fields_survive_round_trip() {
        let spec = AgentSpec {
            name: "rt-agent".to_owned(),
            version: "1.2.3".to_owned(),
            capabilities: vec![
                Capability::new("code", "review"),
                Capability::new("lang", "zh-CN"),
            ],
            accepts: TaskShape {
                description: "PR diff text".to_owned(),
            },
            returns: ResultShape {
                description: "Review comments in Markdown".to_owned(),
            },
            cost_hint: 3.5,
        };

        let store = InMemoryStore::new();
        store.save(&spec).await.unwrap();
        let all = store.load_all().await.unwrap();
        let loaded = &all[0];
        assert_eq!(loaded.name, "rt-agent");
        assert_eq!(loaded.version, "1.2.3");
        assert_eq!(loaded.capabilities.len(), 2);
        assert!((loaded.cost_hint - 3.5).abs() < f64::EPSILON);
    }
}
