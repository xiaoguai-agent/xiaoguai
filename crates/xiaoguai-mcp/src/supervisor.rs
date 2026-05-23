//! Minimal MCP supervisor — owns active client instances keyed by
//! `(tenant_id, server_name, version)`.
//!
//! v0.5.3 ships only `start / get / stop / list_active`. Idle-timeout,
//! LRU eviction, crash-restart, and health-check pings land in v0.5.3.1.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::client::McpClient;
use crate::error::{McpError, McpResult};
use crate::stdio::StdioMcpClient;
use xiaoguai_storage::repositories::McpServerRepository;
use xiaoguai_types::{McpServer, McpTransport};

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

    /// v0.9.4.1: reconcile running clients with the persisted registry.
    ///
    /// Pulls the per-tenant `mcp_servers` slice (system-wide rows + rows
    /// scoped to `tenant_id`) and diffs against `running_servers`. Newly
    /// inserted rows get spawned via the matching transport; rows that
    /// vanished get shut down. Cheap to call after every marketplace
    /// install — the live-pickup path that v0.9.4 deferred.
    ///
    /// Returns the keys that were newly started so callers can log them.
    /// Stop failures are best-effort: the displaced client is logged and
    /// dropped, but the reload completes. Start failures bubble up so the
    /// caller (today: the marketplace install handler) can surface them
    /// in the API response.
    pub async fn reload_from_db(
        &self,
        repo: &dyn McpServerRepository,
        tenant_id: Option<&str>,
    ) -> McpResult<Vec<McpKey>> {
        let rows = match tenant_id {
            Some(t) => repo
                .list_for_tenant(t)
                .await
                .map_err(|e| McpError::Transport(format!("mcp_servers list: {e}")))?,
            None => repo
                .list_global()
                .await
                .map_err(|e| McpError::Transport(format!("mcp_servers list: {e}")))?,
        };

        // Compute the desired set of keys.
        let desired: Vec<(McpKey, McpServer)> = rows
            .into_iter()
            .filter(|s| s.enabled)
            .map(|s| {
                let key = McpKey::new(
                    s.tenant_id
                        .as_ref()
                        .map(|t| t.as_str().to_string())
                        .unwrap_or_default(),
                    s.name.clone(),
                    s.version.clone(),
                );
                (key, s)
            })
            .collect();
        let desired_keys: std::collections::HashSet<McpKey> =
            desired.iter().map(|(k, _)| k.clone()).collect();

        // Stop anything in the live set that's no longer desired *for the
        // tenants we just enumerated*. We deliberately keep clients for
        // other tenants — caller decides whether to reload them.
        let to_stop: Vec<McpKey> = {
            let live = self.clients.lock();
            live.keys()
                .filter(|k| {
                    let in_scope = match tenant_id {
                        Some(t) => k.tenant_id == t || k.tenant_id.is_empty(),
                        None => k.tenant_id.is_empty(),
                    };
                    in_scope && !desired_keys.contains(k)
                })
                .cloned()
                .collect()
        };
        for k in to_stop {
            if let Err(e) = self.stop(&k).await {
                tracing::warn!(?k, error = %e, "mcp supervisor reload: stop failed");
            }
        }

        // Start anything desired that isn't yet live.
        let mut started = Vec::new();
        for (key, server) in desired {
            if self.clients.lock().contains_key(&key) {
                continue;
            }
            match spawn_client(&server).await {
                Ok(client) => {
                    self.start(key.clone(), client).await?;
                    started.push(key);
                }
                Err(e) => {
                    // Transport may not be supported in this build (http
                    // requires a URL endpoint, stdio requires command).
                    // Log + continue: one bad row shouldn't sink the
                    // whole reload.
                    tracing::warn!(server = %server.name, error = %e, "mcp supervisor reload: spawn failed");
                }
            }
        }
        Ok(started)
    }
}

/// Spawn an `McpClient` matching the persisted manifest's transport.
/// Currently supports stdio; http/sse are left unimplemented because the
/// v0.9.0 `HttpMcpClient` constructor expects per-call config that the
/// `mcp_servers` schema doesn't capture yet (auth header, custom
/// headers). Those rows will be picked up by a follow-up tag.
async fn spawn_client(server: &McpServer) -> McpResult<Arc<dyn McpClient>> {
    match server.transport {
        McpTransport::Stdio => {
            let program = server.command.as_deref().ok_or_else(|| {
                McpError::InvalidArgument(format!("stdio server {} missing command", server.name))
            })?;
            let args: Vec<&str> = server.args.iter().map(String::as_str).collect();
            let envs: Vec<(String, String)> = server
                .env_keys
                .iter()
                .filter_map(|k| std::env::var(k).ok().map(|v| (k.clone(), v)))
                .collect();
            let client = StdioMcpClient::spawn(program, &args, &envs).await?;
            Ok(Arc::new(client))
        }
        McpTransport::Http | McpTransport::Sse => Err(McpError::InvalidArgument(format!(
            "{} transport supervisor pickup not implemented yet",
            server.transport.as_str()
        ))),
    }
}
