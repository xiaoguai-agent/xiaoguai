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
use xiaoguai_mcp::McpSupervisor;
use xiaoguai_storage::repositories::{McpServerRepository, MessageRepository, SessionRepository};

use crate::audit::{AuditChainExporter, AuditReader, AuditVerifier};
use crate::auth::TokenValidator;
use crate::eval::EvalService;
use crate::hotl::audit::HotlAuditSink;
use crate::hotl::decision::HotlDecisionStore;
use crate::hotl::decision_registry::DecisionRegistry;
use crate::hotl::enforcer::HotlEnforcer;
use crate::hotl::policy::HotlPolicyStore;
use crate::outcomes::{OutcomeWriter, OutcomesReader};
use crate::scheduler::{
    NlJobCompiler, ScheduledJobUpserter, ScheduledJobsReader, WebhookPusher, WebhookTokenAdmin,
    WebhookTokenValidator,
};
use crate::sessions_ext::SessionForker;
use crate::skills::SkillPackRepository;
use crate::skills_rescan::PackRescanner;
use crate::today::TodayReader;
use crate::usage::UsageReader;
use crate::workspaces::WorkspaceRepository;
use xiaoguai_memory::MemoryStore;
use xiaoguai_personas::{PersonaRepository, TeamRepository};

/// Registry of cancellation tokens keyed by `session_id` — one token per
/// in-flight turn, and (since the /loop L1 prerequisite work) the
/// server-side per-session turn lock: an occupied entry means a turn is
/// running, and [`CancelRegistry::try_begin_turn`] refuses to start another.
///
/// Historical note: turn serialisation used to be a CLIENT convention only
/// ("wait for SSE close before sending the next message") and `register`
/// silently evicted the prior token — two concurrent turns on one session
/// raced each other's finalize/persist. The lock-or-refuse semantics fix
/// that race at its root: the token lifetime IS the lock lifetime.
#[derive(Default)]
pub struct CancelRegistry {
    inner: Mutex<HashMap<String, CancellationToken>>,
}

impl CancelRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin a turn: mint a fresh token if (and only if) no turn is in
    /// flight for `session_id`. Returns `None` when the session is busy —
    /// the route maps this to 409, the loop controller skips the tick.
    ///
    /// The returned [`TurnGuard`] releases the entry on drop, so the lock
    /// survives panics in the finalize task.
    pub fn try_begin_turn(self: &Arc<Self>, session_id: impl Into<String>) -> Option<TurnGuard> {
        use std::collections::hash_map::Entry;
        let session_id = session_id.into();
        let token = CancellationToken::new();
        match self.inner.lock().entry(session_id.clone()) {
            Entry::Occupied(_) => return None,
            Entry::Vacant(v) => {
                v.insert(token.clone());
            }
        }
        Some(TurnGuard {
            registry: Arc::clone(self),
            session_id,
            token,
        })
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

    #[must_use]
    pub fn is_active(&self, session_id: &str) -> bool {
        self.inner.lock().contains_key(session_id)
    }
}

/// RAII handle for one in-flight turn. Holds the session's cancellation
/// token and the per-session turn lock; dropping it releases both.
pub struct TurnGuard {
    registry: Arc<CancelRegistry>,
    session_id: String,
    token: CancellationToken,
}

impl TurnGuard {
    /// Clone of this turn's cancellation token — pass it to the runtime.
    #[must_use]
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        self.registry.inner.lock().remove(&self.session_id);
    }
}

impl std::fmt::Debug for TurnGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnGuard")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
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
    /// Optional MCP registry — when `None` the `/v1/mcp/servers` endpoint
    /// returns 503.
    pub mcp_servers: Option<Arc<dyn McpServerRepository>>,
    /// `None` = auth disabled (handlers fall back to owner identity, fine
    /// for a localhost dev run). `Some(...)` = require a matching
    /// `Authorization: Basic` username/password on `/v1/**` (DEC-033).
    pub auth: Option<Arc<dyn TokenValidator>>,
    /// `None` = `/v1/admin/audit` returns 503. `Some(...)` exposes the
    /// HMAC-chained audit log; production wires the
    /// `xiaoguai-audit::SqliteAuditSink` reader.
    pub audit: Option<Arc<dyn AuditReader>>,
    /// v0.6.5: `None` = `/v1/admin/audit/verify` returns 503.
    /// `Some(...)` exposes per-tenant chain integrity verification;
    /// production wires `SqliteAuditSink` (which implements both reader and
    /// verifier behind the same sink).
    pub audit_verifier: Option<Arc<dyn AuditVerifier>>,
    /// v1.5 (T5): `None` = `POST /v1/audit/exports` returns 503.
    /// `Some(...)` exposes compliance bundle export (SOC2 / GDPR / HIPAA)
    /// over a time window. The adapter re-verifies chain continuity inside
    /// the window and refuses if broken — there is no `skip_verify` flag.
    pub audit_chain_exporter: Option<Arc<dyn AuditChainExporter>>,
    /// v0.9.1: when true, mount `/v1/mcp/serve` so external agents can
    /// consume xiaoguai's `Toolbox` over Streamable HTTP. Default off —
    /// publishing tools is an explicit operator decision.
    pub mcp_publish_enabled: bool,
    /// v0.9.4.1: live `McpSupervisor` so marketplace installs can spawn
    /// the new server immediately (instead of waiting for the next
    /// process restart). `None` keeps the historical write-only
    /// behaviour for callers that haven't wired a supervisor yet (every
    /// existing test uses `None`).
    pub mcp_supervisor: Option<Arc<McpSupervisor>>,
    /// v0.11.1: composite read across chat / IM / scheduled sources used
    /// by `GET /v1/admin/today`, the audit-first console's landing pane.
    /// `None` makes the endpoint return 503 — production wires a
    /// `SqliteTodayReader` adapter in `xiaoguai-core`.
    pub today: Option<Arc<dyn TodayReader>>,
    /// v0.11.2: eval pane substrate — runner + case-from-session
    /// source + suites directory. `None` makes every `/v1/admin/eval/*`
    /// route return 503; production wires it from `[eval]` config.
    pub eval: Option<Arc<EvalService>>,
    /// v0.12.0: scheduler webhook pusher. `None` makes
    /// `POST /v1/admin/scheduler/webhooks/:route_id` return 503;
    /// production wires it from `xiaoguai-core` by wrapping the
    /// running `WebhookSource`. Behind admin auth — per-tenant tokens
    /// land in v0.12.1.
    pub webhook_pusher: Option<Arc<dyn WebhookPusher>>,
    /// v0.12.1: natural-language → `ScheduledJob` compiler. `None`
    /// makes `POST /v1/admin/scheduler/jobs/compile` return 503;
    /// production wires `LlmNlJobCompiler` from `xiaoguai-core` when
    /// an `LlmBackend` is available.
    pub nl_job_compiler: Option<Arc<dyn NlJobCompiler>>,
    /// v0.12.1: scheduled-job upsert sink for `POST /v1/admin/scheduler/jobs`.
    /// `None` makes the endpoint return 503; production wires the
    /// `SqliteJobRepository` via a thin adapter in `xiaoguai-core`.
    pub job_upserter: Option<Arc<dyn ScheduledJobUpserter>>,
    /// v1.1.2: conversation fork — backs
    /// `POST /v1/sessions/:id/fork`. `None` makes the route return
    /// 503; production wires `SqliteSessionForker` in
    /// `xiaoguai-core::sessions_bridge`.
    pub session_forker: Option<Arc<dyn SessionForker>>,
    /// v1.1.1: token-usage aggregator backing `GET /v1/usage`. `None`
    /// makes the endpoint return 503; production wires a
    /// `SqliteUsageReader` in `xiaoguai-core/src/usage_bridge.rs`.
    pub usage_reader: Option<Arc<dyn UsageReader>>,
    /// v0.12.x.1: per-tenant webhook token validator backing
    /// `POST /v1/scheduler/webhooks/:route_id` (note: NOT under /admin —
    /// the admin route stays bearer-gated). `None` makes the public
    /// webhook endpoint return 503; production wires
    /// `SqliteWebhookTokenValidator` from `xiaoguai-core`.
    pub webhook_token_validator: Option<Arc<dyn WebhookTokenValidator>>,
    /// v0.12.x.1: admin CRUD for webhook tokens backing
    /// `/v1/admin/scheduler/tokens`. `None` makes the admin endpoints
    /// return 503.
    pub webhook_token_admin: Option<Arc<dyn WebhookTokenAdmin>>,
    /// v0.12.x.1: read-only enumeration + `fire_now` for the admin-ui
    /// Scheduler pane's Jobs tab. `None` makes
    /// `GET /v1/admin/scheduler/jobs` and the matching `/fire-now`
    /// endpoint return 503.
    pub scheduler_jobs_reader: Option<Arc<dyn ScheduledJobsReader>>,
    /// v1.2.3: HOTL boundary policy store — backs
    /// `GET|POST|DELETE /v1/hotl/policies`. `None` makes the endpoints
    /// return 503; production wires a `SqliteHotlPolicyStore` in
    /// `xiaoguai-core`.
    pub hotl_policy_store: Option<Arc<dyn HotlPolicyStore>>,
    /// v1.2.3: HOTL budget enforcer called from gated action sites
    /// (LLM call path wired; email/webhook deferred). `None` disables
    /// enforcement (allow-all passthrough).
    pub hotl_enforcer: Option<Arc<dyn HotlEnforcer>>,
    /// v1.8.x sprint-11 (S11-3a.1): record-of-decision store backing
    /// `POST /v1/hotl/decisions`. `None` makes the endpoint return 503;
    /// production wires `SqliteHotlDecisionStore` from `xiaoguai-core`.
    ///
    /// 3a.1 ships the decision-record + `raise_policy` route only — the
    /// agent loop does NOT suspend on `Escalate` yet, so the response's
    /// `resumed` field is always `false`. Full suspend/resume
    /// (`SuspendingHotlGate`, `AgentEvent::HotlPending`, `DecisionRegistry`)
    /// is deferred to a future sprint.
    pub hotl_decision_store: Option<Arc<dyn HotlDecisionStore>>,
    /// v1.9.x sprint-12 (S12-3): per-`escalation_id` waiter map for
    /// suspended HOTL requests. Always present — the registry has zero
    /// side-effects when no one calls `register`, so unwiring would just
    /// remove an unused field rather than disable a feature. Shared
    /// between the gate adapter (S12-4) and `POST /v1/hotl/decisions`
    /// (S12-6).
    pub decision_registry: Arc<DecisionRegistry>,
    /// v1.8.x sprint-11 (S11-3a.1): HMAC-chained audit sink for the
    /// HOTL decision route. `None` makes the route skip audit logging
    /// (best-effort — audit failures must NOT block the operation).
    /// Distinct from `audit` (read-only) and `audit_chain_exporter`
    /// (compliance export); production wires a thin adapter around
    /// `xiaoguai_audit::SqliteAuditSink`.
    pub hotl_audit: Option<Arc<dyn HotlAuditSink>>,
    /// v1.2.4: outcome telemetry write side — backs `POST /v1/outcomes`.
    /// `None` makes the endpoint return 503; production wires
    /// `SqliteOutcomeRecorder` via an adapter in `xiaoguai-core`.
    pub outcome_writer: Option<Arc<dyn OutcomeWriter>>,
    /// v1.2.4: outcome telemetry read side — backs
    /// `GET /v1/outcomes/summary` and `GET /v1/outcomes/timeseries`.
    /// `None` makes both endpoints return 503.
    pub outcomes_reader: Option<Arc<dyn OutcomesReader>>,
    /// v1.2.28: skill pack install/uninstall store backing
    /// `GET /v1/skills/installed`, `POST /v1/skills/install`, and
    /// `DELETE /v1/skills/install/:id`. `None` makes those endpoints
    /// return 503; production wires `SqliteSkillPackRepository` from
    /// `xiaoguai-core`.
    pub skill_packs: Option<Arc<dyn SkillPackRepository>>,
    /// v1.3.x: long-term memory with semantic retrieval — backs
    /// `/v1/memories` CRUD + `/v1/memories/recall` + `/v1/memories/similar/:id`.
    /// `None` makes those endpoints return 503; production wires
    /// `SqliteMemoryStore` from `xiaoguai-core`.
    pub memory_store: Option<Arc<dyn MemoryStore>>,
    /// v1.3.x: workspace CRUD backing `GET|POST|PUT|DELETE /v1/workspaces`.
    /// `None` makes those endpoints return 503; production wires
    /// `SqliteWorkspaceRepository` from `xiaoguai-core/src/workspace_bridge.rs`.
    pub workspace_repository: Option<Arc<dyn WorkspaceRepository>>,
    /// v1.5.x (Tier-2 D.1): persistence for agent-authored skill
    /// proposals. `None` makes `/v1/skills/proposals/*` endpoints return
    /// 503; production wires `SqliteSkillProposalRepository`.
    pub skill_proposals: Option<Arc<dyn xiaoguai_tasks::skill_author::SkillProposalRepository>>,
    /// v1.5.x: per-tenant opt-in flag store backing
    /// `allow_skill_authoring`. `None` → `propose_skill` is unavailable.
    pub tenant_settings: Option<Arc<dyn xiaoguai_tasks::skill_author::TenantSettingsReader>>,
    /// v1.5.x: `HotL` gate adapter the `propose_skill` tool consults.
    /// `None` → the routes return 503.
    pub skill_author_gate: Option<Arc<dyn xiaoguai_tasks::skill_author::SkillAuthorGate>>,
    /// v1.5.x: audit sink that records `skill.propose`,
    /// `skill.hotl_gate`, `skill.approve`, `skill.reject`. Production
    /// wires the `SqliteAuditSink` from `xiaoguai-audit`.
    pub skill_audit: Option<Arc<dyn xiaoguai_tasks::skill_author::SkillAuditSink>>,
    /// v1.5.x: directory the approved skill manifests are written to.
    /// Defaults to `~/.xiaoguai/skills` in production wiring.
    pub skills_dir: std::path::PathBuf,
    /// v1.8.0 (sprint-10b S10b-1): persona CRUD + session-attachment store —
    /// backs `/v1/personas/*` and `/v1/sessions/:id/persona`. `None` makes
    /// those endpoints return 503; production wires `SqlitePersonaRepository`
    /// from `xiaoguai-personas`.
    pub personas: Option<Arc<dyn PersonaRepository>>,
    /// T3 expert center: team CRUD + session-attachment store — backs
    /// `/v1/teams/*` and `/v1/sessions/:id/team`. `None` makes those
    /// endpoints return 503; production wires `SqliteTeamRepository`
    /// from `xiaoguai-personas::teams`.
    pub teams: Option<Arc<dyn TeamRepository>>,
    /// T3 expert center: best-effort append-only audit sink for `team.create`
    /// / `team.update` / `team.archive` / `team.attach` entries. Reuses the
    /// generic [`HotlAuditSink`] append trait; production wires the same
    /// `SqliteHotlAuditSink` adapter as `hotl_audit`. `None` skips audit
    /// logging (audit failures must NOT block the operation).
    pub team_audit: Option<Arc<dyn HotlAuditSink>>,
    /// T6 self-healing (GLUE-1): incident persistence — backs
    /// `/v1/incidents/*`. `None` makes those endpoints return 503;
    /// production wires `SqliteIncidentStore` over the shared pool in
    /// `xiaoguai-core`. Ingest audit reuses `team_audit` (the sink is
    /// feature-generic; entries differ only by action namespace).
    pub incidents: Option<Arc<dyn crate::incident_store::IncidentStore>>,
    /// v1.8.0 (sprint-10b S10b-5): session-scoped watcher introspection —
    /// backs `/v1/watchers/*`. `None` makes those endpoints return 503;
    /// production wires `StaticWatcherIntrospector` (zero-watcher steady
    /// state) until a session-aware `xiaoguai-watch::WatchRunner`
    /// introspection adapter ships.
    pub watchers: Option<Arc<dyn crate::watchers::WatcherIntrospector>>,
    /// /loop L1 (DEC-039): session-scoped recurring agent turns — backs
    /// `/v1/loops/*`. `None` makes those endpoints return 503; production
    /// wires a `LoopController` over `SqliteLoopRepository` in
    /// `xiaoguai-core::run_serve` (the controller holds an `AppState`
    /// clone captured BEFORE this field is set, so it never re-enters
    /// itself).
    pub loops: Option<Arc<crate::loops::LoopController>>,
    /// Phase 5 (skill-pack loader): hot-activate an installed conversational
    /// pack's agent team without rebooting `serve` — backs
    /// `POST /v1/admin/skills/rescan`. `None` makes that route return 503
    /// (a non-`packs` build, or a deployment without the persona/team repos);
    /// production wires a bridge in `xiaoguai_core::run_serve` that closes over
    /// the `SQLite` pool + the live persona/team repos and calls
    /// `pack_runtime::scan_enabled_pack_agents`. The boot scan still covers
    /// anomaly/watch specs — only the conversational team is hot-rescanned.
    pub pack_rescanner: Option<Arc<dyn PackRescanner>>,
    /// Feature ⑤ (per-session coding workspace root): rebuilds the agent
    /// toolbox with the governed coding tools rooted at a session's
    /// `working_dir` for a single turn. `None` when coding is not enabled at
    /// boot (no audit key / no global `XIAOGUAI_CODING_WORKSPACE`), or for any
    /// `AppState` built without the bridge (every test) — then `run_turn`
    /// always uses `toolbox` as-is, exactly as before this field existed.
    /// Production wires `CodingToolboxFactoryImpl` from
    /// `xiaoguai_core::coding_bridge` only when the boot toolbox already
    /// carries coding tools, so the opt-in/security posture is unchanged.
    pub coding_toolbox_factory: Option<Arc<dyn crate::coding_toolbox::CodingToolboxFactory>>,
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
    fn turn_guard_round_trip() {
        let reg = Arc::new(CancelRegistry::new());
        let guard = reg.try_begin_turn("sess_1").expect("first turn starts");
        let tok = guard.token();
        assert!(!tok.is_cancelled());
        assert!(reg.is_active("sess_1"));
        assert!(reg.cancel("sess_1"));
        assert!(tok.is_cancelled());
        drop(guard);
        assert!(!reg.is_active("sess_1"));
    }

    #[test]
    fn second_turn_refused_while_first_in_flight() {
        let reg = Arc::new(CancelRegistry::new());
        let guard = reg.try_begin_turn("sess_1").expect("first turn starts");
        assert!(
            reg.try_begin_turn("sess_1").is_none(),
            "concurrent turn on the same session must be refused"
        );
        // A different session is unaffected.
        assert!(reg.try_begin_turn("sess_2").is_some());
        drop(guard);
        assert!(
            reg.try_begin_turn("sess_1").is_some(),
            "lock releases when the guard drops"
        );
    }

    #[test]
    fn cancel_returns_false_for_unknown_session() {
        let reg = CancelRegistry::new();
        assert!(!reg.cancel("nope"));
    }
}
