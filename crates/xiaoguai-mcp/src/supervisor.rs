//! Minimal MCP supervisor — owns active client instances keyed by
//! `(server_name, version)`.
//!
//! v0.5.3 ships only `start / get / stop / list_active`. Idle-timeout,
//! LRU eviction, crash-restart, and health-check pings land in v0.5.3.1.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;

use crate::client::McpClient;
use crate::error::{McpError, McpResult};
use crate::stdio::StdioMcpClient;
use crate::types::ToolDescriptor;
use xiaoguai_storage::repositories::McpServerRepository;
use xiaoguai_types::{McpServer, McpTransport};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct McpKey {
    pub server_name: String,
    pub version: String,
}

impl McpKey {
    #[must_use]
    pub fn new(server_name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            server_name: server_name.into(),
            version: version.into(),
        }
    }
}

/// One running server: its client plus the tool descriptors captured once at
/// `start` time. Caching the descriptors at spawn keeps the per-turn merge
/// (`active_tools`) a cheap lock+clone — it never re-runs `list_tools()`, so
/// it can't add an MCP round-trip or hang to a chat turn.
struct ActiveServer {
    client: Arc<dyn McpClient>,
    /// Tools the server advertised at `start`. Empty when the one-shot
    /// `list_tools()` failed (best-effort — a tool-less server never breaks
    /// registration, it simply contributes nothing to the toolbox).
    tools: Vec<ToolDescriptor>,
}

#[derive(Default)]
pub struct McpSupervisor {
    clients: Mutex<HashMap<McpKey, ActiveServer>>,
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

/// Per-turn seam (testable): the source of every active server's
/// `(client, descriptor)` pairs the turn pipeline merges into its toolbox.
/// [`McpSupervisor`] is the production implementation; tests use a mock so the
/// merge path is exercisable without spawning real MCP child processes.
#[async_trait]
pub trait ActiveToolsSource: Send + Sync {
    /// Snapshot of every active server's tools, each paired with the client
    /// that dispatches it. Implementations must be cheap and non-blocking on
    /// the common path — the turn pipeline calls this once per turn.
    async fn active_tools(&self) -> Vec<(Arc<dyn McpClient>, ToolDescriptor)>;
}

impl McpSupervisor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a client under `key`. If a client is already registered under
    /// that key it's replaced and best-effort-shutdown.
    ///
    /// The client's tools are enumerated ONCE here (`list_tools()`) and cached
    /// so the per-turn [`Self::active_tools`] path stays a cheap lock+clone and
    /// can never add an MCP round-trip — or hang — to a chat turn. A
    /// `list_tools()` failure is best-effort: it is logged and the server
    /// registers with an empty tool set (it just contributes nothing to the
    /// agent's toolbox) rather than failing the spawn.
    ///
    /// # Errors
    /// This function currently always returns `Ok(())`. The signature is
    /// `McpResult` for forward-compatibility with richer start logic.
    pub async fn start(&self, key: McpKey, client: Arc<dyn McpClient>) -> McpResult<()> {
        // Cache descriptors at spawn time (await BEFORE taking the lock — the
        // parking_lot guard is not held across the round-trip).
        let tools = match client.list_tools().await {
            Ok(tools) => tools,
            Err(e) => {
                tracing::warn!(
                    server = %key.server_name,
                    version = %key.version,
                    error = %e,
                    "mcp supervisor: list_tools at start failed; server registers with no tools",
                );
                Vec::new()
            }
        };
        let prev = self
            .clients
            .lock()
            .insert(key, ActiveServer { client, tools });
        if let Some(p) = prev {
            // Best-effort: log on failure but never propagate, since the
            // caller is registering a fresh client.
            if let Err(e) = p.client.shutdown().await {
                tracing::warn!(error = %e, "displaced MCP client shutdown failed");
            }
        }
        Ok(())
    }

    /// Cheap `Arc` clone of the active client for `key`, if any.
    #[must_use]
    pub fn get(&self, key: &McpKey) -> Option<Arc<dyn McpClient>> {
        self.clients.lock().get(key).map(|s| s.client.clone())
    }

    /// Remove and shut down. Idempotent: missing key is a successful no-op.
    ///
    /// # Errors
    /// Returns `McpError` if the client's shutdown implementation fails.
    pub async fn stop(&self, key: &McpKey) -> McpResult<()> {
        let removed = self.clients.lock().remove(key);
        if let Some(s) = removed {
            s.client.shutdown().await?;
        }
        Ok(())
    }

    /// Snapshot of registered keys. Order is unspecified.
    #[must_use]
    pub fn list_active(&self) -> Vec<McpKey> {
        self.clients.lock().keys().cloned().collect()
    }

    /// Every active server's cached tools, each paired with the client that
    /// dispatches it. Cheap: a single lock plus `Arc`/descriptor clones, no
    /// `list_tools()` round-trip (descriptors were cached at [`Self::start`]).
    /// Returns an empty `Vec` when no servers are active, so the per-turn
    /// merge can short-circuit without allocating a toolbox.
    #[must_use]
    pub fn active_tools_snapshot(&self) -> Vec<(Arc<dyn McpClient>, ToolDescriptor)> {
        let guard = self.clients.lock();
        let mut out = Vec::new();
        for server in guard.values() {
            for descriptor in &server.tools {
                out.push((server.client.clone(), descriptor.clone()));
            }
        }
        out
    }

    /// v0.9.4.1: reconcile running clients with the persisted registry.
    ///
    /// Pulls the `mcp_servers` slice and diffs against the running set.
    /// Newly inserted rows get spawned via the matching transport; rows
    /// that vanished get shut down. Cheap to call after every marketplace
    /// install — the live-pickup path that v0.9.4 deferred.
    ///
    /// Returns the keys that were newly started so callers can log them.
    /// Stop failures are best-effort: the displaced client is logged and
    /// dropped, but the reload completes. Start failures bubble up so the
    /// caller (today: the marketplace install handler) can surface them
    /// in the API response.
    ///
    /// # Errors
    /// Returns `McpError::Transport` if the repository query fails, or
    /// propagates `McpError` from `start` if a newly-spawned client fails
    /// to register.
    pub async fn reload_from_db(&self, repo: &dyn McpServerRepository) -> McpResult<Vec<McpKey>> {
        let rows = repo
            .list()
            .await
            .map_err(|e| McpError::Transport(format!("mcp_servers list: {e}")))?;

        // Compute the desired set of keys.
        let desired: Vec<(McpKey, McpServer)> = rows
            .into_iter()
            .filter(|s| s.enabled)
            .map(|s| {
                let key = McpKey::new(s.name.clone(), s.version.clone());
                (key, s)
            })
            .collect();
        let desired_keys: std::collections::HashSet<McpKey> =
            desired.iter().map(|(k, _)| k.clone()).collect();

        // Stop anything in the live set that's no longer desired.
        let to_stop: Vec<McpKey> = {
            let live = self.clients.lock();
            live.keys()
                .filter(|k| !desired_keys.contains(k))
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

#[async_trait]
impl ActiveToolsSource for McpSupervisor {
    async fn active_tools(&self) -> Vec<(Arc<dyn McpClient>, ToolDescriptor)> {
        // Pure delegation to the cheap, cached snapshot — no awaiting work,
        // but the trait is `async` so a future live-fetching source (e.g. an
        // HTTP supervisor) can satisfy it without an API change.
        self.active_tools_snapshot()
    }
}

/// #286: env-var allowlist of MCP server **names** whose self-declared
/// `readOnlyHint` annotations are trusted (comma-separated, e.g.
/// `XIAOGUAI_MCP_TRUST_READ_ONLY_HINTS=github,docs-search`). Any server
/// not on the list gets every tool classified `MutationHint::Write`, so
/// consult (read-only) mode excludes and denies it.
///
/// TODO(#286): promote this to a `trust_read_only_hints` column on the
/// `mcp_servers` table once a migration is scheduled — the env allowlist
/// keeps the opt-in per-server without touching the schema today.
pub const XIAOGUAI_MCP_TRUST_READ_ONLY_HINTS_ENV: &str = "XIAOGUAI_MCP_TRUST_READ_ONLY_HINTS";

/// Parse the allowlist value into trusted server names. Pure (env-free)
/// so it stays unit-testable without process-global env races.
fn parse_trusted_servers(raw: &str) -> std::collections::HashSet<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// #286: does the operator trust `server_name`'s `readOnlyHint`s?
/// Absent/empty env var → trust nobody (fail-closed default).
fn server_trusts_read_only_hints(server_name: &str) -> bool {
    std::env::var(XIAOGUAI_MCP_TRUST_READ_ONLY_HINTS_ENV)
        .map(|raw| parse_trusted_servers(&raw).contains(server_name))
        .unwrap_or(false)
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
            // #286: per-server opt-in — only allowlisted servers get their
            // self-declared readOnlyHint honored (consult-mode eligibility).
            let trust = server_trusts_read_only_hints(&server.name);
            if trust {
                tracing::info!(
                    server = %server.name,
                    "mcp: readOnlyHint trusted via {XIAOGUAI_MCP_TRUST_READ_ONLY_HINTS_ENV}"
                );
            }
            let client = StdioMcpClient::spawn(program, &args, &envs)
                .await?
                .with_trusted_read_only_hints(trust);
            Ok(Arc::new(client))
        }
        McpTransport::Http | McpTransport::Sse => Err(McpError::InvalidArgument(format!(
            "{} transport supervisor pickup not implemented yet",
            server.transport.as_str()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // #286: allowlist parsing — pure function, no env mutation (parallel
    // test safety).

    #[test]
    fn trusted_servers_parse_trims_and_skips_empties() {
        let set = parse_trusted_servers(" github , ,docs-search,");
        assert_eq!(set.len(), 2);
        assert!(set.contains("github"));
        assert!(set.contains("docs-search"));
    }

    #[test]
    fn trusted_servers_empty_string_trusts_nobody() {
        assert!(parse_trusted_servers("").is_empty());
        assert!(parse_trusted_servers("  ,  ").is_empty());
    }

    #[test]
    fn trusted_servers_exact_name_match_only() {
        let set = parse_trusted_servers("github");
        assert!(set.contains("github"));
        // No prefix/substring matching — "github-evil" is NOT trusted.
        assert!(!set.contains("github-evil"));
        assert!(!set.contains("git"));
    }
}
