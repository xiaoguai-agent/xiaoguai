//! Minimal MCP supervisor — owns active client instances keyed by
//! `(tenant_id, server_name, version)`.
//!
//! v0.5.3 ships only `start / get / stop / list_active`. Idle-timeout,
//! LRU eviction, crash-restart, and health-check pings land in v0.5.3.1.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::client::McpClient;
use crate::error::McpResult;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct McpKey {
    /// Empty string = global (system-wide) — chosen sentinel so the type
    /// stays `Hash + Eq` without needing `Option` plumbing in the map.
    pub tenant_id: String,
    pub server_name: String,
    pub version: String,
}

impl McpKey {
    #[must_use]
    pub fn new(
        tenant_id: impl Into<String>,
        server_name: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            server_name: server_name.into(),
            version: version.into(),
        }
    }

    /// Shorthand for "system-wide" keys (`tenant_id = ""`).
    #[must_use]
    pub fn global(server_name: impl Into<String>, version: impl Into<String>) -> Self {
        Self::new("", server_name, version)
    }
}

#[derive(Default)]
pub struct McpSupervisor {
    clients: Mutex<HashMap<McpKey, Arc<dyn McpClient>>>,
}

impl std::fmt::Debug for McpSupervisor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpSupervisor")
            .field(
                "active",
                &self.clients.lock().keys().cloned().collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl McpSupervisor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a client under `key`. If a client is already registered under
    /// that key it's replaced and best-effort-shutdown.
    pub async fn start(&self, key: McpKey, client: Arc<dyn McpClient>) -> McpResult<()> {
        let prev = self.clients.lock().insert(key, client);
        if let Some(p) = prev {
            // Best-effort: log on failure but never propagate, since the
            // caller is registering a fresh client.
            if let Err(e) = p.shutdown().await {
                tracing::warn!(error = %e, "displaced MCP client shutdown failed");
            }
        }
        Ok(())
    }

    /// Cheap `Arc` clone of the active client for `key`, if any.
    #[must_use]
    pub fn get(&self, key: &McpKey) -> Option<Arc<dyn McpClient>> {
        self.clients.lock().get(key).cloned()
    }

    /// Remove and shut down. Idempotent: missing key is a successful no-op.
    pub async fn stop(&self, key: &McpKey) -> McpResult<()> {
        let removed = self.clients.lock().remove(key);
        if let Some(c) = removed {
            c.shutdown().await?;
        }
        Ok(())
    }

    /// Snapshot of registered keys. Order is unspecified.
    #[must_use]
    pub fn list_active(&self) -> Vec<McpKey> {
        self.clients.lock().keys().cloned().collect()
    }
}
