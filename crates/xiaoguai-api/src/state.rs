//! Application state shared across all axum handlers.
//!
//! v0.5.5 keeps state minimal: repository handles, a single `LlmBackend`
//! (the multi-backend `LlmRouter` already implements `LlmBackend` via the
//! trait, so production wiring substitutes it transparently), the shared
//! `Toolbox`, agent defaults, and a per-session cancellation registry.
//!
//! Auth context, per-tenant routing, and RBAC enforcement are tracked in
//! v0.5.5.1 — they need request-scope plumbing that doesn't belong inside
//! `AppState`.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_llm::LlmBackend;
use xiaoguai_storage::repositories::{MessageRepository, SessionRepository};

/// Registry of cancellation tokens keyed by `session_id`. A single token per
/// session is enough — the API contract serialises message turns within a
/// session (the client should wait for SSE close before sending the next one).
#[derive(Default)]
pub struct CancelRegistry {
    inner: Mutex<HashMap<String, CancellationToken>>,
}

impl CancelRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a token, evicting any prior one. Returns the freshly inserted
    /// clone so the caller can use it as their cancellation source of truth.
    pub fn register(&self, session_id: impl Into<String>) -> CancellationToken {
        let token = CancellationToken::new();
        let mut g = self.inner.lock();
        g.insert(session_id.into(), token.clone());
        token
    }

    /// Cancel a session in-flight. Returns `true` if a token was found.
    pub fn cancel(&self, session_id: &str) -> bool {
        if let Some(t) = self.inner.lock().get(session_id) {
            t.cancel();
            true
        } else {
            false
        }
    }

    /// Drop the registry entry for `session_id`. Should be called once the
    /// loop finishes (success or error) to avoid leaking tokens.
    pub fn drop_entry(&self, session_id: &str) {
        self.inner.lock().remove(session_id);
    }

    #[must_use]
    pub fn is_active(&self, session_id: &str) -> bool {
        self.inner.lock().contains_key(session_id)
    }
}

#[derive(Clone)]
pub struct AppState {
    pub sessions: Arc<dyn SessionRepository>,
    pub messages: Arc<dyn MessageRepository>,
    pub backend: Arc<dyn LlmBackend>,
    pub toolbox: Arc<Toolbox>,
    pub agent_defaults: AgentConfig,
    pub cancels: Arc<CancelRegistry>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("backend", &self.backend.name())
            .field("toolbox_size", &self.toolbox.len())
            .field("agent_defaults", &self.agent_defaults)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_registry_round_trip() {
        let reg = CancelRegistry::new();
        let tok = reg.register("sess_1");
        assert!(!tok.is_cancelled());
        assert!(reg.is_active("sess_1"));
        assert!(reg.cancel("sess_1"));
        assert!(tok.is_cancelled());
        reg.drop_entry("sess_1");
        assert!(!reg.is_active("sess_1"));
    }

    #[test]
    fn cancel_returns_false_for_unknown_session() {
        let reg = CancelRegistry::new();
        assert!(!reg.cancel("nope"));
    }
}
