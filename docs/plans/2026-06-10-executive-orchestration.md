# Implementation plan — T4: Executive routing + parallel orchestration + synthesis

| | |
|---|---|
| Date | 2026-06-10 |
| Status | **APPROVED (owner blanket "全部执行" 2026-06-10)** — open questions flagged in the PR, not blocking |
| Parent | `docs/plans/2026-06-09-capability-upgrade.md` §2-D / §3-T4 (second wave) |
| Hard constraints | DEC-033 unchanged: 单二进制 · 内嵌 SQLite · 单 owner · `:7600` |

## 0. Goal & scope

Give a team a real execution model: **goal in → members run in parallel →
lead synthesizes one answer out**, governed end-to-end (HotL + audit +
attribution), productized as a session API.

**Size discipline (M):** this deliberately does NOT add multi-persona session
state. An orchestrated run happens *inside one session turn*: member persona
runs are ephemeral in-process agent runs (triangle-worker-style); only the
**synthesized text** is persisted as the session's assistant reply. Member
transcripts surface through the SSE event stream + audit, not `messages`.
Per-persona session message logs / multi-persona `session_personas` are out of
scope (revisit if a real need appears).

## 1. What exists (verified, explore report 2026-06-10)

- **Triangle pattern is serial** (`crates/xiaoguai-orchestrator/src/patterns/
  triangle.rs` run_loop; `supervisor.rs:21` explicitly defers parallel
  dispatch). Reusable shapes: `TriangleRunner::stream()` → mpsc
  `OrchEvent` stream; `TriangleBudget::split`; scratchpad quarantine.
- **In-process turn primitive exists**: `xiaoguai-runtime::run_streamed(ctx,
  history, cancel)` → `(JoinHandle<RuntimeOutcome>, ReceiverStream<AgentEvent>)`;
  `RuntimeContext::with_model/with_attribution`; the ReAct loop already
  HotL-gates every tool call (`react.rs`, scope `tool_call.{name}`) and stamps
  token usage from `AgentConfig.session_id` (the loop/scheduler/IM synthetic-
  label precedent: `scheduler:<job_id>`, `im:<provider>:<conv>`).
- **Turn locking**: `run_turn` (`crates/xiaoguai-api/src/turn.rs`) acquires the
  per-session lock via `CancelRegistry::try_begin_turn` → 409 on collision.
- **Routing inputs**: T3 shipped teams (`lead_persona_id` + ordered
  `member_persona_ids`) and the deterministic suggest scorer; persona
  enforcement helpers (`build_system_messages`, `filter_tools`).
- **CapabilityRouter** (`registry/router.rs`) remains unwired; its exact
  AND-coverage matching fits explicit-capability intents — NOT used here
  (same call as T3's suggest; revisit when personas grow a `capabilities`
  field).

## 2. Design

### 2.1 Orchestrator crate — `patterns/executive.rs` (new, LLM-free)

```rust
pub struct MemberSpec { pub id: Uuid, pub name: String /* opaque to runner */ }
pub struct MemberOutcome { pub id: Uuid, pub ok: bool, pub text: String,
                           pub iterations: u32 }

#[async_trait]
pub trait MemberRunner: Send + Sync {
    /// Run one member persona against the goal; full agent turn, tools allowed.
    async fn run_member(&self, member: &MemberSpec, goal: &str)
        -> Result<MemberOutcome, OrchestratorError>;
    /// Run the lead synthesis turn over the members' outcomes.
    async fn run_synthesis(&self, lead: &MemberSpec, goal: &str,
                           outcomes: &[MemberOutcome])
        -> Result<String, OrchestratorError>;
}

pub struct ExecutiveRunner<R: MemberRunner> { /* members, lead, runner, caps */ }
pub enum ExecEvent {
    RunStarted { members: usize },
    MemberStarted { id: Uuid }, MemberCompleted { id: Uuid, ok: bool },
    SynthesisStarted { ok_members: usize },
    Final { ok: bool, text: String, failed_members: Vec<Uuid> },
}
impl<R> ExecutiveRunner<R> {
    pub fn stream(self, goal: String) -> impl Stream<Item = ExecEvent>;
}
```

- Members run **concurrently** (`join_all`), each isolated (no shared
  scratch — quarantine by construction). A failed member does not abort the
  run; synthesis receives the survivors (≥1 required, else `Final{ok:false}`).
- `max_members` cap (default 8) enforced at construction.
- Synthesis prompt contract (built by the runner impl): goal + each member's
  name + outcome text; instructed to surface inter-member disagreements
  explicitly rather than papering over them (v1 conflict handling; the
  registry `ConflictArbitrator` stays for resource locks, unused here).
- Mock-runner unit tests in the crate: parallel fan-out, partial failure,
  all-fail, member cap, event ordering.

### 2.2 API layer — routing + the real runner + route

- **Routing = API layer** (it owns personas/teams/scorer): request names a
  `team_id` (explicit) or `goal`-only (auto → top team from the T3 suggest
  scorer; 422 when nothing matches). Resolved team → lead + member `Persona`s
  (active only).
- `OrchestrateMemberRunner` (in `xiaoguai-api`, mirrors how `LoopController`
  composes turns): per member builds a `RuntimeContext` with the persona's
  `system_prompt` (via `build_system_messages`), `default_model`, toolbox
  filtered by `filter_tools`, the session's HotL gate inherited from
  `agent_defaults`, and **attribution label `orch:<run_id>:<persona_id>`**
  (disjoint from `sess_*`, follows the scheduler/IM precedent — budget sums by
  exact match stay unaffected).
- **Route**: `POST /v1/sessions/{id}/orchestrate` body
  `{ goal, team_id?, max_members? }` → SSE stream of `ExecEvent` frames.
  - Holds the **session turn lock** for the whole run (409 `turn_in_flight`
    on collision — an orchestrated run IS the session's turn).
  - HotL: turn-level `enforcer.check("llm_call", member_count + 1)` up front
    (fail-closed, same as `run_turn`); per-tool gates apply inside each member
    run automatically.
  - Persistence: user goal stored as the user message; **synthesized text
    stored as the assistant reply**; both via the existing message repo path.
  - Audit: `orchestration.start` (team, members, goal hash) and
    `orchestration.complete` (ok, failed members, per-member token attribution
    pointers) through the `team_audit` sink pattern.
  - 503-when-absent: requires `personas` + `teams`.
- **Client**: shared `orchestrateSession(sessionId, req)` returning an SSE
  reader consistent with `sendMessage`'s event-stream handling.

### 2.3 Deferred to T5 (explicitly)

Chat-ui surfacing (a "team run / consult vs execute" mode control) lands with
T5's consult/execute split — one UI for both concerns. T4 ships engine + API +
client.

## 3. Task breakdown (TDD; clippy `-D warnings` + nextest green per task)

| # | Task | Size | Verification |
|---|---|---|---|
| T4.1 | `patterns/executive.rs` + mock-runner tests | M | unit: fan-out, partial fail, cap, ordering |
| T4.2 | API: routing + `OrchestrateMemberRunner` + route + persistence + audit | M | route tests: happy path (mock backend), 409 lock, 422 no-match, 503, audit entries, attribution labels |
| T4.3 | shared client method + tests; docs (user guide §, plan link) | S | client unit tests; tsc |

## 4. Boundaries

- No session schema changes; no multi-persona `session_personas`.
- No `CapabilityRouter` wiring; no `capabilities` persona field.
- No cross-member shared scratchpad; no replan loop (one round + synthesis —
  replan/critic belongs to the triangle pattern, not the executive pattern).
- No new crates; DEC-033 intact.

## 5. Open questions (flagged in PR, defaults chosen)

1. **Auto-routing default**: goal-only requests auto-pick the top suggest-
   scored team. Default ON (422 when no match) — owner can demand explicit
   `team_id` only.
2. **Member failure policy**: continue-with-survivors (chosen) vs all-or-
   nothing.
3. **Token budget**: v1 governs via HotL `llm_call` amount + iteration caps +
   post-hoc attribution; a hard per-run token ceiling can reuse the /loop
   `session_total_since` machinery later if needed.
