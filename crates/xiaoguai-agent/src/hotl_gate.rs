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
//! * `Suspend { request_id, scope, ticket }` (sprint-12) → do NOT dispatch
//!   yet. The loop must emit `AgentEvent::HotlPending`, then call
//!   `ticket.await_decision(&cancel)` to receive an operator verdict from
//!   `DecisionRegistry`. Only the new `SuspendingHotlGate`
//!   (see `xiaoguai-core::hotl_bridge`) ever emits this variant; today's
//!   `EnforcerGate` keeps mapping upstream `Escalate` to `Allow` for
//!   backward compatibility (`agent.hotl.suspend_on_escalate=false`).
//! * Infrastructure error → fail-closed: same as `Deny` plus a
//!   `tracing::error` emission. The adapter living in `xiaoguai-core`
//!   maps the upstream enforcer error into this verdict; the trait here
//!   stays infallible at the surface so the loop has one branch.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::oneshot;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Decision returned by [`HotlGate::check`].
///
/// Note: `Clone`/`PartialEq` are NOT derived because the sprint-12
/// `Suspend` variant holds a non-cloneable `oneshot::Receiver`. The
/// existing `Allow` and `Deny` variants remain cloneable/comparable via
/// the manual impls below; cloning a `Suspend` verdict panics — callers
/// must consume the ticket exactly once, which matches the loop's
/// single-await semantics.
#[derive(Debug)]
pub enum HotlGateVerdict {
    /// Budget within limits — proceed with the tool dispatch.
    Allow,
    /// Budget breached or infrastructure failure (fail-closed). The caller
    /// must NOT dispatch the tool; the `reason` is surfaced to the LLM.
    Deny(String),
    /// Sprint-12 (S12-1, additive). Tool dispatch is suspended pending an
    /// operator decision. The caller must emit
    /// `AgentEvent::HotlPending { request_id, scope, ... }` and then call
    /// `ticket.await_decision(&cancel)`. The resolved verdict either
    /// authorises the dispatch (Allow), synthesises a failed `ToolResult`
    /// (Deny / Timeout), or surrenders to a parent cancel
    /// (`HotlTicketError::Cancelled`).
    ///
    /// This variant is **only** emitted by `SuspendingHotlGate` in
    /// `xiaoguai-core::hotl_bridge`, which is selected per-tenant via the
    /// `agent.hotl.suspend_on_escalate` config flag. The legacy
    /// `EnforcerGate` continues to map upstream `Escalate` → `Allow` so
    /// existing tenants observe no behaviour change until S12-12 flips
    /// the default in v1.9.0.
    Suspend {
        request_id: uuid::Uuid,
        scope: String,
        ticket: HotlSuspensionTicket,
    },
}

impl Clone for HotlGateVerdict {
    /// Cloning `Allow` and `Deny` is byte-identical to the pre-sprint-12
    /// derived impl. Cloning a `Suspend` verdict panics — the ticket is
    /// a one-shot resource and must be consumed exactly once. The loop
    /// never clones a verdict (it matches and moves), so this branch is
    /// unreachable in practice; the panic exists only to satisfy callers
    /// (like test fixtures) that hold `Clone` verdicts statically.
    fn clone(&self) -> Self {
        match self {
            Self::Allow => Self::Allow,
            Self::Deny(reason) => Self::Deny(reason.clone()),
            Self::Suspend { .. } => panic!(
                "HotlGateVerdict::Suspend cannot be cloned — the suspension ticket is one-shot. \
                 Match-and-move the verdict instead of cloning."
            ),
        }
    }
}

impl PartialEq for HotlGateVerdict {
    /// `Allow == Allow` and `Deny(r1) == Deny(r2)` iff `r1 == r2`.
    /// Two `Suspend` verdicts are never equal (the tickets are distinct
    /// one-shot resources). This matches the v1.8.x behaviour for
    /// `Allow`/`Deny` comparisons (the only pair tests have ever used).
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Allow, Self::Allow) => true,
            (Self::Deny(a), Self::Deny(b)) => a == b,
            _ => false,
        }
    }
}

/// Sprint-12 (S12-1). Outcome of an operator's `HotL` decision, returned
/// by [`HotlSuspensionTicket::await_decision`] when the operator (or the
/// timeout helper) sends a verdict through the registry's `oneshot`
/// sender. Mirrors the wire shape of `POST /v1/hotl/decisions` in
/// `api-contract.md` §2.6.2 for the fields the agent loop consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotlDecisionVerdict {
    /// The operator's resolution (or `Timeout` synthesised by the
    /// registry's expiry helper).
    pub verdict: HotlResolution,
    /// The operator that recorded the decision, if known. `None` when
    /// the verdict is `Timeout` (no operator was involved) or when the
    /// authentication identity is not yet wired (sprint-13 follow-up).
    pub decided_by: Option<String>,
    /// Wall-clock timestamp the verdict was committed at. Sourced from
    /// the resolver (`DecisionRegistry::resolve`) on the API side or from
    /// the ticket's timeout helper.
    pub recorded_at: chrono::DateTime<chrono::Utc>,
}

/// Sprint-12 (S12-1). The three terminal states of a suspended tool
/// call. Mirrors `api-contract.md` §2.6.3 `hotl_resolved.verdict` enum
/// and is what the ReAct loop matches in its `HotlGateVerdict::Suspend`
/// arm (added by S12-5) to decide whether to dispatch the tool, deny it,
/// or annotate it as timed-out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotlResolution {
    /// Operator approved the tool call. Loop must dispatch.
    Allow,
    /// Operator rejected the tool call. Loop must synthesise a failed
    /// `ToolResult` carrying the reason so the LLM observes the denial.
    Deny(String),
    /// `expires_at` passed without an operator decision. Equivalent in
    /// effect to `Deny("operator decision timed out")` from the loop's
    /// perspective, but distinguished on the SSE wire (`verdict=timeout`)
    /// so the frontend can render the dedicated annotation.
    Timeout,
}

/// Sprint-12 (S12-1). Error returned by
/// [`HotlSuspensionTicket::await_decision`] when the wait could not
/// complete because the parent operation was cancelled, or because the
/// registry dropped its sender without sending a verdict (which would be
/// a registry bug — included for forward compatibility).
#[derive(Debug, thiserror::Error)]
pub enum HotlTicketError {
    /// The caller's `CancellationToken` fired before a verdict arrived
    /// and before the timeout expired. The loop's iteration-boundary
    /// cancel logic emits `Final(Cancelled)`; the suspend arm must NOT
    /// emit `HotlResolved` in this path.
    #[error("HotL suspension cancelled by parent operation")]
    Cancelled,
    /// The registry's sender was dropped without sending a verdict. In
    /// production this should never happen — the registry owns the
    /// sender, only releases it on `resolve()` (which sends), or on the
    /// timeout helper (which sends `Timeout`). Surfaced as an error so
    /// the loop can degrade gracefully rather than hang forever.
    #[error("HotL decision channel was dropped before a verdict was sent")]
    ChannelDropped,
}

/// Sprint-12 (S12-1). One-shot ticket returned inside
/// `HotlGateVerdict::Suspend`. Holds the receiver half of a `oneshot`
/// channel paired with the deadline `expires_at`. The sender half lives
/// in `DecisionRegistry` (S12-3) keyed by `request_id`; the route handler
/// (`POST /v1/hotl/decisions`, S12-6) resolves it on operator decision,
/// and the registry's companion timeout future resolves it with
/// `HotlResolution::Timeout` if `expires_at` elapses first.
///
/// Use [`HotlSuspensionTicket::await_decision`] to consume the ticket.
/// The function takes `self` (drop-on-await) because the receiver is a
/// one-shot resource — you can wait on it exactly once.
#[derive(Debug)]
pub struct HotlSuspensionTicket {
    rx: oneshot::Receiver<HotlDecisionVerdict>,
    expires_at: Instant,
    /// Same `request_id` the loop emits on the matching
    /// `AgentEvent::HotlPending`. Exposed so the loop can include it in
    /// `HotlResolved` events without threading it through a separate
    /// channel.
    pub request_id: uuid::Uuid,
}

impl HotlSuspensionTicket {
    /// Sprint-12 (S12-5). Read-only accessor for the deadline. The loop
    /// needs this BEFORE consuming the ticket via `await_decision` so it
    /// can derive the `expires_at: DateTime<Utc>` field of the
    /// `AgentEvent::HotlPending` it emits to SSE clients.
    #[must_use]
    pub fn expires_at(&self) -> Instant {
        self.expires_at
    }

    /// Sprint-12 (S12-3 calls this). Constructs a ticket together with
    /// its paired sender. The registry stores `sender` keyed by
    /// `request_id` and hands the ticket back to the gate, which embeds
    /// it inside `HotlGateVerdict::Suspend`.
    ///
    /// The pair is returned so the registry can also spawn a companion
    /// `tokio::time::sleep_until(expires_at)` task that sends a
    /// `Timeout` verdict if no operator decision arrives in time.
    /// `await_decision` independently sleeps to `expires_at`, so even
    /// without the registry's helper the loop still observes a timeout
    /// (defence in depth).
    #[must_use]
    pub fn new(
        request_id: uuid::Uuid,
        expires_at: Instant,
    ) -> (Self, oneshot::Sender<HotlDecisionVerdict>) {
        let (tx, rx) = oneshot::channel();
        let ticket = Self {
            rx,
            expires_at,
            request_id,
        };
        (ticket, tx)
    }

    /// Wait for the operator's decision, the configured timeout, or a
    /// parent cancellation — whichever happens first.
    ///
    /// Returns:
    /// - `Ok(verdict)` when the registry's sender sends, OR when the
    ///   internal `sleep_until(expires_at)` fires (synthesised as
    ///   `HotlResolution::Timeout` with `decided_by: None`).
    /// - `Err(HotlTicketError::Cancelled)` when `cancel` fires before
    ///   either of the above. The caller (the ReAct loop) is responsible
    ///   for NOT emitting `HotlResolved` in this path — the parent cancel
    ///   logic emits `Final(Cancelled)` instead.
    /// - `Err(HotlTicketError::ChannelDropped)` when the registry's
    ///   sender is dropped without sending. Should not happen in
    ///   production; surfaced so a misconfigured registry cannot make
    ///   the loop hang forever.
    pub async fn await_decision(
        self,
        cancel: &CancellationToken,
    ) -> Result<HotlDecisionVerdict, HotlTicketError> {
        let Self { rx, expires_at, .. } = self;
        tokio::select! {
            // Bias the select so cancellation always wins ties — this matches
            // the loop's documented "cancel wins" semantics from DEC-LLD-AGENT-004
            // and is what S12-9's hotl_suspend_cancel.rs integration test will pin.
            biased;
            () = cancel.cancelled() => Err(HotlTicketError::Cancelled),
            res = rx => match res {
                Ok(verdict) => Ok(verdict),
                Err(_recv_err) => Err(HotlTicketError::ChannelDropped),
            },
            () = tokio::time::sleep_until(expires_at) => Ok(HotlDecisionVerdict {
                verdict: HotlResolution::Timeout,
                decided_by: None,
                recorded_at: chrono::Utc::now(),
            }),
        }
    }
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
