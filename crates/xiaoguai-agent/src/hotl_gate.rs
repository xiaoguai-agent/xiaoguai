//! Tool-call HOTL gate — the abstract interface the ReAct loop consults
//! before dispatching each MCP tool call.
//!
//! ## Why a local trait (not `xiaoguai_api::hotl::enforcer::HotlEnforcer`)?
//!
//! `xiaoguai-api` already depends on `xiaoguai-agent` (it builds
//! `ReactAgent` from `AppState`). Letting `xiaoguai-agent` import
//! `HotlEnforcer` from `xiaoguai-api` would create a dependency cycle.
//!
//! Instead, `xiaoguai-agent` defines a minimal [`HotlGate`] trait scoped
//! to what the loop actually needs: take `(tenant_id, scope, amount)`,
//! return `Allow` or `Deny(reason)`. `xiaoguai-core` (which depends on
//! both crates) ships the adapter that implements `HotlGate` on top of
//! the full `HotlEnforcer` — see `xiaoguai-core::hotl_bridge::EnforcerGate`.
//!
//! ## Design choice (option a vs b)
//!
//! Two options were on the table:
//!   * (a) Pass `Option<Arc<dyn HotlGate>>` into `AgentConfig` and consult it
//!     per tool call in the dispatch loop.
//!   * (b) Wrap each `McpClient` in a `HotlGatedClient` decorator.
//!
//! We picked **(a)**: simpler (no per-client wrapping ceremony), keeps the
//! enforcer signal centralised next to where the dispatch fans out, and
//! the same loop already owns the `tenant_id` and the `ToolCallSpec.name`
//! needed to compute the scope. The decorator pattern would have spread
//! budget logic across every `Toolbox::register` site and lost the
//! "per call is a budget event" guarantee for parallel dispatch.
//!
//! ## Semantics
//!
//! Per tool call (LLM may emit several per turn):
//!
//! * `scope` = `format!("tool_call.{tool_name}")`
//! * `amount` = `1.0` (count-only enforcement; token attribution comes later)
//! * `tenant_id` = the parsed `Uuid` from `AgentConfig::tenant_id`. If the
//!   `tenant_id` is absent or unparseable, the gate is skipped (no policy
//!   scope to enforce against).
//!
//! Verdict handling:
//!
//! * `Allow` → dispatch the tool as today.
//! * `Deny(reason)` → do NOT dispatch; synthesise a failed `ToolResult`
//!   so the LLM observes the denial and can adapt.
//! * Infrastructure error → fail-closed: same as `Deny` plus a
//!   `tracing::error` emission. The adapter living in `xiaoguai-core`
//!   maps the upstream enforcer error into this verdict; the trait here
//!   stays infallible at the surface so the loop has one branch.

use std::sync::Arc;

use async_trait::async_trait;

/// Decision returned by [`HotlGate::check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotlGateVerdict {
    /// Budget within limits — proceed with the tool dispatch.
    Allow,
    /// Budget breached or infrastructure failure (fail-closed). The caller
    /// must NOT dispatch the tool; the `reason` is surfaced to the LLM.
    Deny(String),
}

/// Abstract HOTL budget gate consulted per tool call by the ReAct loop.
///
/// Implementations should record the event before returning (optimistic
/// insert) so concurrent callers see a consistent tally, matching the
/// contract of `xiaoguai_api::hotl::enforcer::HotlEnforcer`.
///
/// The trait surface is infallible — adapters map upstream errors into
/// `Deny(reason)` (fail-closed). This keeps the loop's branch count low
/// and matches the security-first posture: any uncertainty about the
/// budget state must produce a deny, never silently allow.
#[async_trait]
pub trait HotlGate: Send + Sync + std::fmt::Debug {
    async fn check(&self, tenant_id: uuid::Uuid, scope: &str, amount: f64) -> HotlGateVerdict;
}

/// Convenience type alias for the optional gate plugged into `AgentConfig`.
pub type SharedHotlGate = Arc<dyn HotlGate>;

// ── test stubs ──────────────────────────────────────────────────────────────

/// Always-allow gate. Used in unit tests where the gate must be present
/// but should never block.
#[derive(Debug, Default, Clone)]
pub struct AllowAllGate;

#[async_trait]
impl HotlGate for AllowAllGate {
    async fn check(&self, _tenant: uuid::Uuid, _scope: &str, _amount: f64) -> HotlGateVerdict {
        HotlGateVerdict::Allow
    }
}

/// Always-deny gate. Used in unit tests to assert the tool was NOT
/// dispatched and the denial reason flows back to the LLM.
#[derive(Debug, Clone)]
pub struct DenyAllGate {
    pub reason: String,
}

impl DenyAllGate {
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

#[async_trait]
impl HotlGate for DenyAllGate {
    async fn check(&self, _tenant: uuid::Uuid, _scope: &str, _amount: f64) -> HotlGateVerdict {
        HotlGateVerdict::Deny(self.reason.clone())
    }
}

/// Per-scope deny gate. Allows everything except the named scope(s).
/// Lets tests prove that gating is per-tool (the second of two tool calls
/// can be denied while the first executes).
#[derive(Debug, Clone)]
pub struct ScopeDenyGate {
    pub deny_scopes: Vec<String>,
    pub reason: String,
}

impl ScopeDenyGate {
    #[must_use]
    pub fn new(deny_scopes: Vec<String>, reason: impl Into<String>) -> Self {
        Self {
            deny_scopes,
            reason: reason.into(),
        }
    }
}

#[async_trait]
impl HotlGate for ScopeDenyGate {
    async fn check(&self, _tenant: uuid::Uuid, scope: &str, _amount: f64) -> HotlGateVerdict {
        if self.deny_scopes.iter().any(|s| s == scope) {
            HotlGateVerdict::Deny(self.reason.clone())
        } else {
            HotlGateVerdict::Allow
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::Instant;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    #[tokio::test]
    async fn allow_all_returns_allow() {
        let g = AllowAllGate;
        let v = g.check(Uuid::new_v4(), "tool_call.search", 1.0).await;
        assert_eq!(v, HotlGateVerdict::Allow);
    }

    #[tokio::test]
    async fn deny_all_returns_deny_with_reason() {
        let g = DenyAllGate::new("budget exceeded");
        let v = g.check(Uuid::new_v4(), "tool_call.search", 1.0).await;
        assert_eq!(v, HotlGateVerdict::Deny("budget exceeded".into()));
    }

    #[tokio::test]
    async fn scope_deny_is_selective() {
        let g = ScopeDenyGate::new(vec!["tool_call.execute_python".into()], "no python in prod");
        let allowed = g.check(Uuid::new_v4(), "tool_call.search", 1.0).await;
        assert_eq!(allowed, HotlGateVerdict::Allow);
        let denied = g
            .check(Uuid::new_v4(), "tool_call.execute_python", 1.0)
            .await;
        assert_eq!(denied, HotlGateVerdict::Deny("no python in prod".into()));
    }

    // ── S12-1 sprint-12: HotlSuspensionTicket tests ─────────────────────────

    #[tokio::test]
    async fn ticket_resolves_when_sender_sends_allow() {
        let request_id = Uuid::new_v4();
        let expires_at = Instant::now() + Duration::from_secs(60);
        let (ticket, sender) = HotlSuspensionTicket::new(request_id, expires_at);

        assert_eq!(ticket.request_id, request_id);

        let verdict = HotlDecisionVerdict {
            verdict: HotlResolution::Allow,
            decided_by: Some("alice@example.com".into()),
            recorded_at: chrono::Utc::now(),
        };
        sender.send(verdict.clone()).expect("send must succeed");

        let cancel = CancellationToken::new();
        let got = ticket
            .await_decision(&cancel)
            .await
            .expect("ticket must resolve");
        assert_eq!(got.verdict, HotlResolution::Allow);
        assert_eq!(got.decided_by.as_deref(), Some("alice@example.com"));
    }

    #[tokio::test]
    async fn ticket_times_out_at_expires_at() {
        let request_id = Uuid::new_v4();
        let expires_at = Instant::now() + Duration::from_millis(50);
        let (ticket, _sender) = HotlSuspensionTicket::new(request_id, expires_at);

        let cancel = CancellationToken::new();
        let start = std::time::Instant::now();
        let got = ticket
            .await_decision(&cancel)
            .await
            .expect("timeout path is Ok(verdict=Timeout), not Err");
        let elapsed = start.elapsed();

        assert_eq!(got.verdict, HotlResolution::Timeout);
        assert_eq!(got.decided_by, None);
        assert!(
            elapsed >= Duration::from_millis(45),
            "must wait until expires_at"
        );
    }

    #[tokio::test]
    async fn ticket_cancels_when_token_fires() {
        let request_id = Uuid::new_v4();
        let expires_at = Instant::now() + Duration::from_secs(60);
        let (ticket, _sender) = HotlSuspensionTicket::new(request_id, expires_at);

        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancel_clone.cancel();
        });

        let err = ticket
            .await_decision(&cancel)
            .await
            .expect_err("cancel must surface as Err");
        assert!(matches!(err, HotlTicketError::Cancelled));
    }

    #[tokio::test]
    async fn ticket_returns_channel_dropped_when_sender_dropped() {
        let request_id = Uuid::new_v4();
        let expires_at = Instant::now() + Duration::from_secs(60);
        let (ticket, sender) = HotlSuspensionTicket::new(request_id, expires_at);

        drop(sender);

        let cancel = CancellationToken::new();
        let err = ticket
            .await_decision(&cancel)
            .await
            .expect_err("dropped sender must surface as Err");
        assert!(matches!(err, HotlTicketError::ChannelDropped));
    }
}
