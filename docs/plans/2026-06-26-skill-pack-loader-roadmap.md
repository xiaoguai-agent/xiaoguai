# Skill Pack Loader — Remaining Roadmap (Phase 4d + Phase 5 completion)

**Status:** DESIGN — review checkpoint (2026-06-26). No code is written. This doc opens the **three pieces that remain** after Phase 4a/4b shipped and Phase 4c (chat-ui UX) + the Phase 5 rescan endpoint went in-flight:

1. **Phase 4d-a — inline pack-tool execution** (compile an agent's inline `tools[]` into agent-callable tools).
2. **Phase 4d-b — reactive / triggered workers** (bind `triggers[]` agents to the event/watch bus).
3. **Phase 5 completion** — beyond the in-flight team hot-rescan: (a) **anomaly/watch hot-rescan** (the registries+scheduler live in `run_serve`, not on `AppState`) and (b) **auto-activate-on-install** (CLI best-effort pokes the running serve).

**Context.** The loader shipped Phase 1 (`pack validate`, #336/#338/#344), Phase 2 (watch/anomaly execution, v1.26.0, #352/#353), Phase 4a (pure parse + plan, `crates/xiaoguai-core/src/pack_agents.rs`, #360), and Phase 4b (boot-scan activates conversational agent *teams*, #361 — `crates/xiaoguai-core/src/pack_runtime.rs:456 scan_enabled_pack_agents`). Phase 4c (surface activated teams in chat + onboarding) is in-flight on `feat/skill-pack-phase4c` (`3ba6849`, chat-ui only). The Phase 4 parent design ([2026-06-25-skill-pack-loader-phase4.md](2026-06-25-skill-pack-loader-phase4.md)) explicitly **deferred** inline pack tools (§C4) and reactive workers (§C3) to "Phase 4b/its-own-doc" — **this doc is that doc** (named 4d here to avoid colliding with the already-shipped boot-scan that the parent loosely called "4b"). Phase 3 (per-pack SQL migrations) stays deferred — the [corpus disposition](2026-06-24-skill-pack-corpus-disposition.md) found the shipped packs target operator-owned schemas, so they are **templates**, not runnable-here.

> Methodology (per the repo's "verify before citing in design docs" rule, same as the parent doc): every code fact below was grep-checked on 2026-06-26 against `main` (`e76b31b`) and is labelled **V** with a `file:line` citation. Recommendations are **REC**. §F is an honest scope/risk + open-questions section.

---

## A. Where the loader stands today (grep-checked 2026-06-26)

### A1. What already runs — **V**
- **Anomaly/watch specs run** (Phase 2): `PackAnomalyExecutor` / `PackWatchExecutor` (`crates/xiaoguai-core/src/pack_runtime.rs:60,144`) are registered into the `CompositeExecutor` (`lib.rs:557-580`) and fire as `ScheduledJob`s the boot-scan upserts (`lib.rs:589,614`).
- **Conversational agent *teams* run** (Phase 4b): `scan_enabled_pack_agents` (`pack_runtime.rs:456`) upserts one managed `Persona` per conversational agent + a `Team`, called once at boot (`lib.rs:943-960`). The existing `POST /v1/sessions/{id}/orchestrate` path then runs the team unchanged (parent doc §B2/§C).
- **`pack validate` reports the plan** including what *won't* activate (`crates/xiaoguai-cli/src/commands/pack.rs:79,350 render_agent_plan`).

### A2. What is parsed-but-not-run — **V** (the subject of this doc)
- **Inline `tools[]` bodies.** `AgentDef.tools: Vec<AgentTool>` keeps only `{name, kind}` (`pack_agents.rs:48,60-64`); the `query` / `.j2` / `output:` bodies are **dropped by serde** ("everything else is parsed-and-ignored" — `pack_agents.rs:25-26`). `DerivedPersona.declared_tools` carries only names (`pack_agents.rs:97-99`), and `activate_pack_team` sets `tool_allowlist: None` — *"Inline pack tools are v1-deferred … the agent reasons with platform tools"* (`pack_runtime.rs:515-518`). A validate warning fires per agent (`pack_agents.rs:174-181`).
- **Reactive `triggers[]` agents.** `classify()` routes any agent with `triggers[]` to `AgentRole::Reactive` (`pack_agents.rs:78-84`); `plan_pack_agents` pushes them onto `skipped_reactive` and never builds a persona for them (`pack_runtime.rs:166-170` → `pack_agents.rs:166-170`). The validate report prints *"N event-triggered agent(s) not activated in v1"* (`pack.rs:367-373`).

### A3. The activation seam — **V**
- Install records `{enabled, pack_dir}` into `installed_skill_packs.config` (CLI: `pack.rs:105-123`; out-of-process — writes SQLite directly, **no serve call**). Phase 4b additionally writes back `agents:[...]` (`pack_runtime.rs:572-596 record_activated_agents`) so the marketplace API can flip `activation_status → active` and list the agents.

---

## B. Verified runtime surface for the three pieces (grep-checked 2026-06-26)

### B1. The orchestrate member's toolset — the 4d-a registration target — **V**
- `OrchestrateMemberRunner` holds a shared `toolbox: Arc<Toolbox>` (`crates/xiaoguai-api/src/orchestrate.rs:108`); each member's toolset is **`subset_toolbox(base, persona)`** (`orchestrate.rs:261`), which `filter_tools(persona, &available)` then rebuilds a `Toolbox` from the **persona's `tool_allowlist`** (`orchestrate.rs:256-264`; `xiaoguai_personas::filter_tools`). The model is `persona.default_model` else the session model (`orchestrate.rs:186`).
- `Toolbox` is the agent tool registry (`crates/xiaoguai-agent/src/toolbox.rs:42`). **So a pack-defined tool reaches a member iff (i) it is registered in the base `Toolbox` and (ii) the member persona's `tool_allowlist` names it.** Both are missing today (allowlist is `None`, toolbox holds only platform tools).

### B2. There is no template engine in the workspace — **V**
- No `minijinja` / `tera` / `handlebars` dependency anywhere (grep across `crates/` returns only unrelated `.j2`-mentioning eval/orchestrator test fixtures, never a renderer). **Phase 4d-a's template tools require introducing a (single, vetted) templating crate** — a real new dependency to weigh against DEC-033's "don't add deps casually."

### B3. The event bus — the 4d-b binding target — **V**
- `event_channel() -> (EventSender, EventReceiver)` (`crates/xiaoguai-scheduler/src/trigger_source.rs:95`); `trait TriggerSource` (`:116`) is the `start(&self, tx)`-shaped producer abstraction. `run_serve` already drives one webhook source + (optionally) a file-watch source into a single `event_rx` (`lib.rs:666-695`).
- The consumer is **`JobRunner::fire_event(TriggerEvent)`** (`crates/xiaoguai-scheduler/src/runner.rs` `fire_event` — looks up the job by `event.job_id`, skips missing/disabled, fires with retries). `spawn_file_watch_source` (`crates/xiaoguai-core/src/scheduler_bridge.rs:435`) is the **template** for adding a new `TriggerSource` that feeds `event_tx`. So a reactive worker = a `ScheduledJob` whose `Trigger` is *event* (not cron/interval) + a source that emits `TriggerEvent{job_id}` when the subscribed event happens.

### B4. The scheduler is repo-driven each tick — **V** (decisive for Phase 5)
- `JobRunner::tick()` calls `self.jobs.list_due(now, …)` **every tick** (`runner.rs` `tick`); it holds **no in-memory job list**. `SqliteJobRepository::upsert` persists to the `scheduled_jobs` table (`crates/xiaoguai-scheduler/src/sqlite_repository.rs:35-72`) and `list_due` re-reads it (`:79-104`). **⟹ Re-upserting a `pack.*` job mid-run makes the next tick pick it up automatically — no runner restart, no runner handle needed.**
- `AppState` already exposes a job upserter: `job_upserter: Option<Arc<dyn ScheduledJobUpserter>>` (`crates/xiaoguai-api/src/state.rs:200`), wired from `SqliteScheduledJobUpserter` (`lib.rs:717`). So the *scheduled-jobs* half of a rescan is reachable from a route **today**.

### B5. …but the **anomaly registry + watch dedup are NOT on `AppState`** — **V** (the real Phase 5 gap)
- The shared `Arc<Mutex<AnomalyRegistry>>` (`lib.rs:548-553`) and the `Arc<DedupCache>` (`lib.rs:572-575`) are **local variables inside the `if settings.scheduler.enabled` block**. They are moved into the executors and then dropped from scope. **`AppState` has no field for either** (grep of `state.rs` finds `personas`/`teams`/`skill_packs`/`job_upserter` but no `anomaly_registry`/`dedup`).
- This matters because `PackAnomalyExecutor` **only `observe()`s** an already-registered detector (`pack_runtime.rs:113-120`); the baseline must be `register()`ed first (`pack_runtime.rs:295-298`). A new pack's anomaly job could be upserted and would *fire*, but `observe("unknown-spec")` just **warns and returns `None`** (`crates/xiaoguai-anomaly/src/registry.rs:135-138`) — the detector silently never arms. **So upserting the job is necessary but not sufficient; the rescan must also reach the live registry to `register()` the new spec.**
- Good news for clean rescan: `AnomalyRegistry::deregister(id)` exists (`registry.rs:118`), so add/remove on uninstall is supported by the type.

### B6. The personas/teams half of Phase 5 *is* reachable — **V**
- `personas: Option<Arc<dyn PersonaRepository>>` (`state.rs:302`) + `teams: Option<Arc<dyn TeamRepository>>` (`state.rs:307`) are on `AppState`. `scan_enabled_pack_agents(pool, &dyn PersonaRepository, &dyn TeamRepository)` (`pack_runtime.rs:456`) takes exactly those trait objects and is **idempotent** (`pack_runtime.rs:907-924` test). **⟹ The in-flight Phase 5 rescan endpoint can hot-activate teams by calling `scan_enabled_pack_agents` straight from the route — no new plumbing.** This is precisely why teams were the first slice: they need nothing that boot doesn't already give a route.

### B7. CLI install is out-of-process — **V**
- `xiaoguai pack install` (`pack.rs:98`) and `xiaoguai skills install` open the SQLite file and write `installed_skill_packs` directly; **neither contacts a running serve.** A separate `serve` process holds the live registries/teams (§B5/§B6). So "install → live" today requires a manual `serve` restart (the install message literally says *"wire on the next `serve` boot"*, `pack.rs:126-130`). Closing that is Phase 5 (b).

---

## C. Phase 4d-a — inline pack-tool execution

> **Goal.** An agent's inline `tools[]` — `kind: sql` (a parameterized read-only SELECT) or `kind: template` (render a `.j2` → an `output:` adapter) — become **agent-callable tools** the orchestrate member can invoke, instead of being dropped (§A2).

### C1. Parse the tool *bodies* (today only names survive) — REC
Extend `AgentTool` (`pack_agents.rs:60`) to capture the executable body — additively, behind the same tolerant-serde posture:
- `kind: sql` → `{ name, description, query: String, params: Vec<ParamSpec> }` where `ParamSpec{ name, required, … }` declares the named binds the SQL uses (`:since`, `$model`, …).
- `kind: template` → `{ name, description, template_path: String /* .j2 */, output: OutputAdapterRef }`.
This is a **pure parse change** (mirrors how 4a added `AgentDef`) and is independently testable. `DerivedPersona` gains a `tools: Vec<CompiledToolSpec>` alongside the existing `declared_tools` names.

### C2. Compile each spec into a `Toolbox` entry — REC
A new `xiaoguai-core` (or small `xiaoguai-pack-tools`) module builds a `Tool` impl per spec:
- **SQL tool** — `execute(args)` binds `args` to the declared `:params` and runs the query against the **embedded read pool**, returning rows as JSON. **Boundary validation reuses the Phase-2 guard verbatim**: reject anything not starting with `SELECT` (`pack_runtime.rs:90-95,178-184`), bind **only** as parameters (never string-interpolate — the params are named and typed), and run on the read-only pool handle. This is the same "operator-authored but must be a read-only SELECT against the single SQLite" contract Phase 2 already enforces (DEC-033 — `pack_runtime.rs:88-89`).
- **Template tool** — `execute(args)` renders `template_path` with `args` via the §B2 engine, then hands the rendered string to the **output adapter** named by `output:`. v1 supports the in-process adapters only (e.g. write to a pack-scoped table / return inline); network/file adapters are gated behind explicit operator config like the Phase-2 HTTP-watch path (`pack_runtime.rs:190-194`), not auto-enabled.

### C3. Register compiled tools into the member toolset — REC
Two seams, both already exist (§B1):
1. **Build the tools into the base `Toolbox`** the orchestrate runner receives — namespaced `pack:{slug}/{agent}/{tool}` so pack tools never shadow platform tools.
2. **Name them in the persona's `tool_allowlist`** so `subset_toolbox` (`orchestrate.rs:261`) actually exposes them to that member (and *only* that member). Phase 4d-a flips `activate_pack_team`'s `tool_allowlist: None` (`pack_runtime.rs:515`) to the agent's compiled-tool names **plus** the platform tools the author opted into. This is the one behavioural change to the shipped boot-scan.

### C4. Where the compiled tools live across boot + rescan — REC
The base `Toolbox` is constructed in `run_serve` and handed to the orchestrate route. v1 compiles pack tools **at boot** (alongside `scan_enabled_pack_agents`) and on **rescan** (§D). Because the orchestrate runner reads the toolbox by `Arc`, the cleanest design carries the pack-tool layer as an `Arc<RwLock<…>>`-backed registry the rescan can extend — but a simpler v1 (rebuild-on-rescan, see open question F-Q1) is acceptable if it avoids new shared-mutability.

### C5. Honesty + scope — REC
`pack validate` already warns "inline tool(s) … NOT executed in v1" (`pack_agents.rs:174-181`); 4d-a **flips that message** to "would execute N tool(s): …" for the kinds it supports and keeps the warning for unsupported kinds. The UI scope note from the parent doc (§D1) is updated in lockstep.

---

## D. Phase 4d-b — reactive / triggered workers

> **Goal.** Agents carrying `triggers[]` (currently `skipped_reactive`, §A2) come alive as **event-driven workers**, closer to the Phase-2 watch runtime than to session orchestration (parent doc §B6/§C3).

### D1. A reactive worker is a ScheduledJob with an *event* trigger — REC
The verified consumer is `JobRunner::fire_event(TriggerEvent{job_id})` (§B3). So:
- For each reactive agent, derive a `ScheduledJob` with id `pack:{slug}:agent:{name}` whose `Trigger` is **event** (the existing non-scheduled trigger kind `tick()` deliberately skips — `runner.rs:166 tick` → `list_due` filters on `is_scheduled()`, `sqlite_repository.rs:100` / `trigger.rs:207`). Its `payload.kind` is a new `pack.agent` dispatched by the `CompositeExecutor` to a new **`PackAgentExecutor`** (sibling of the Phase-2 executors).
- `PackAgentExecutor::execute(job)` runs **one agent turn** for the triggering event: load the agent's `system_prompt` + (4d-a) its compiled tools, feed the event payload as the user turn, run to a stop reason, dispatch its `on_match`/sinks like the watch executor already does (`pack_runtime.rs:329-335 watch_sinks`). This is an **agent loop per event**, not a session — matching the parent doc's framing (§C3).

### D2. The trigger source bridges the subscribed event → `event_tx` — REC
`triggers[]` entries are `{ event: "events.vip_registration_needs_handling", kind: inbound }` (parent doc §B5). Two cases:
- **`kind: inbound` / event-bus events** — register the `job_id` against the event name in a **pack-event source** modelled on `spawn_file_watch_source` (`scheduler_bridge.rs:435`): a `TriggerSource` that, when something emits that event, sends `TriggerEvent{job_id, detail}` to `event_tx`. The *emitters* are the existing in-process producers (e.g. an anomaly FIRED, an IM inbound) — **v1 wires the subset that already have an in-process emit point**; truly external event sources are out of scope (no new ingress, DEC-033).
- **Poll-shaped triggers** (a row appears) are **already covered by Phase-2 watch** — for those, recommend authors use a `watch` spec, not a reactive agent. 4d-b documents this split rather than duplicating the poll loop.

### D3. Relationship to the watch/scheduler runtime — REC (explicit, per the ask)
- **Reuse, don't fork.** Reactive workers ride the **same `JobRunner` + `scheduled_jobs` table + audit chain + sinks** as Phase-2 watches/anomalies. The *only* new pieces are (i) `PackAgentExecutor` (runs an agent turn) and (ii) a pack-event `TriggerSource` mapping event-name → `job_id`. No second daemon, no second job store (DEC-033, same discipline as Phase 2/4b).
- **Watch vs reactive-agent boundary:** a *watch* polls a source and dedups rows (data → alert); a *reactive agent* reacts to an event and **reasons** (event → agent turn → action). 4d-b is for the latter; the former stays in Phase-2 watch.

### D4. Per-agent HotL / on_failure — REC
The agent YAML carries `hotl` + `on_failure` (parent doc §B5) that v1 dropped. `PackAgentExecutor` is the natural place to honour them, but v1 **keeps the platform-global HotL + audit** (parent doc §C2/§C6) and treats per-agent `hotl`/`on_failure` as a **4d-b stretch**, surfaced in `pack validate` as "not yet enforced." Don't widen scope to a per-agent governance surface in the first slice.

---

## E. Phase 5 completion

> The in-flight Phase 5 PR adds `POST /v1/admin/skills/rescan` that hot-activates **agent teams** (reachable today, §B6). This section designs the **two remaining halves**: anomaly/watch hot-rescan, and auto-activate-on-install.

### E1. (a) Anomaly/watch hot-rescan — carry `Arc` handles on `AppState` — REC
The blocker is §B5: the live `AnomalyRegistry` + `DedupCache` are `run_serve` locals, unreachable from a route. Two options, **REC = option 1**:

- **Option 1 (REC) — put the handles on `AppState`.** Add two optional fields:
  ```
  pack_anomaly_registry: Option<Arc<Mutex<AnomalyRegistry>>>,   // = lib.rs:549 handle
  pack_watch_dedup:      Option<Arc<DedupCache>>,               // = lib.rs:572 handle
  pack_job_upserter:     reuse the existing job_upserter (state.rs:200)
  ```
  populated from the **same `Arc`s** already built at `lib.rs:549,572` (clone before they move into the executors). `None` when `packs`/`scheduler` are disabled → the rescan route degrades to "teams only" (or 503 for the anomaly/watch portion), consistent with how every other optional subsystem degrades (`state.rs` `Option<…>` pattern).
  Then the rescan handler:
  1. `scan_enabled_pack_agents(pool, personas, teams)` — teams (already in-flight).
  2. `scan_enabled_pack_anomalies(pool, &registry)` → upsert returned jobs via `job_upserter`; the scan **`register()`s the new detectors into the live registry** (`pack_runtime.rs:295-298`), which is exactly what `observe()` needs (§B5).
  3. `scan_enabled_pack_watches(pool)` → upsert returned jobs via `job_upserter`. (Watch dedup is per-spec runtime state; new specs need no pre-registration — `pack_runtime.rs:371-383`.)
  Because the runner is repo-driven each tick (§B4), the upserted jobs fire on the next tick with **no runner handle and no restart**.

  > **Idempotency caveat (V).** `scan_enabled_pack_anomalies` is documented **"Call once per process"** because `register()` *overwrites* and would reset a baseline (`pack_runtime.rs:246-248`). A *rescan* must therefore be made **delta-aware**: only `register()` specs **not** already in `registry.registered_ids()` (`registry.rs:160`), and `deregister()` (`registry.rs:118`) specs whose pack was disabled/uninstalled. **REC:** add a `rescan_pack_anomalies` variant (or a `register_if_absent` flag) so re-arming never nukes a warm baseline. This is the single most important correctness point in Phase 5 — call it out in the PR.

- **Option 2 (rejected) — a scheduler "re-register" RPC.** Push a command channel into the runner. More moving parts, and unnecessary given §B4 (the runner already reloads jobs); it would only help the *registry* problem, which option 1 solves more directly. Rejected to keep one mechanism.

### E2. (b) Auto-activate-on-install — CLI best-effort POST to the running serve — REC
Goal: `xiaoguai pack install` → live with no manual step (§B7). Because install is out-of-process, after the SQLite write (`pack.rs:111-123`) the CLI **best-effort** POSTs the running serve's rescan endpoint:
- **Discovery:** default `http://127.0.0.1:7600` (DEC-033 default port), overridable by the existing `--server` / `XIAOGUAI_SERVER`-style flag the CLI already uses for `remote` (`crates/xiaoguai-cli/src/commands/remote.rs:24`). Reuse that resolver — don't invent a new one.
- **Auth:** reuse the same owner-auth header path `remote`/admin calls already carry (the rescan route is owner-gated like the rest of `/v1/admin/*`).
- **Best-effort semantics (REC):** a connect failure (no serve running) is **not** an install error — print *"installed; no running server detected, will activate on next `serve` boot"* and exit 0. A reachable-but-error response is surfaced as a warning. **Never** let the live-poke turn a successful record into a failed install. (Mirrors the Phase-2 "a bad pack must never stop boot" non-fatal discipline, `lib.rs:587-609`.)
- **Symmetry:** `pack uninstall` / `skills install/uninstall` get the same best-effort poke so disable → live-deactivate works too (the rescan's delta logic in E1 handles removal via `deregister`).

### E3. Uninstall / disable on rescan — REC
The rescan is **convergent**, not additive: it should reconcile the live state to the set of *enabled* packs. Teams for disabled packs → archive the managed personas/team (reuse the archived-aware upsert already in `activate_pack_team`, `pack_runtime.rs:496-525`); anomaly detectors → `deregister`; jobs → disable/delete via the upserter. v1 may ship **add-only** first (open question F-Q3) and follow with removal, but the endpoint contract should be "make live match enabled," documented up front.

---

## F. Scope, risks, and open questions for the owner

### Scope cuts held (DEC-033)
- **No new store / daemon / port.** Everything rides the one SQLite, the one `JobRunner`, the one orchestrate engine, `:7600`. Reactive workers reuse the scheduler; inline tools reuse the `Toolbox`; rescan reuses the repo-driven runner.
- **No external ingress.** 4d-b wires only events with an existing in-process emit point; no new network event source.
- **Read-only SQL only.** 4d-a SQL tools reuse the Phase-2 `SELECT`-guard + parameter binding verbatim.

### Risks
1. **New dependency for templating (V, §B2).** 4d-a template tools need a renderer the workspace doesn't have. Weigh one vetted crate (e.g. a minimal Jinja-compatible engine) vs. dropping `kind: template` from v1 and shipping `kind: sql` only. *Recommend: SQL-tools-first slice; template tools behind their own go/no-go.*
2. **Anomaly rescan can reset baselines (V, §E1 caveat).** The shipped scan overwrites on `register`. The delta-aware rescan is mandatory, not optional.
3. **Shared mutability for the live tool registry (§C4).** Extending the base `Toolbox` at rescan time needs either `RwLock` or rebuild-on-rescan. Keep it simple; don't introduce broad interior mutability for a rare operation.
4. **Reactive agent loops are unbounded work (§D1).** An event storm → many agent turns → token spend. Reuse the scheduler's existing per-user/day budget ledger (`runner.rs` `with_budget_ledger`) and surface a per-pack cap.
5. **Corpus reality (carried from parent §G5).** The shipped pack agent YAMLs are foreign-schema templates; 4d-a/4d-b must be demonstrated on a **SQLite-native canonical pack** (the `observability-starter` equivalent for agent tools/triggers), not the corpus.

### Open questions
- **F-Q1 — tool registry mutability:** rebuild-the-`Toolbox`-on-rescan (simple, brief lock) vs. an `Arc<RwLock<PackToolLayer>>` (live, more code)? *Lean: rebuild-on-rescan for v1.*
- **F-Q2 — template tools in or out of the first slice?** Ship `kind: sql` alone first and gate `kind: template` (the new-dependency cost) behind a follow-up? *Lean: yes, SQL-first.*
- **F-Q3 — rescan add-only vs. convergent now?** Ship add-only activation first (lowest risk) and follow with disable/uninstall reconciliation (E3), or build convergent from the start? *Lean: add-only first, with the contract documented as "will become convergent."*
- **F-Q4 — which events does 4d-b subscribe in v1?** Enumerate the in-process emit points worth wiring (anomaly-FIRED? IM-inbound? watch-match?) — owner picks the v1 set; the rest are documented as "needs an emitter."
- **F-Q5 — auto-activate default-on?** Should the install-time live-poke be on by default, or opt-in via a flag for operators who deploy serve and CLI on different hosts? *Lean: on by default, silent no-op when no local serve.*

### Phased, reviewable PRs (TDD, each gated on Build-and-test)
- **P5 — Phase 5 completion (smallest, do first; finishes the in-flight rescan).** `AppState` carries the anomaly-registry + dedup `Arc`s (§E1 option 1); rescan handler calls all three scans; **delta-aware anomaly re-arm** (§E1 caveat); CLI best-effort install/uninstall poke (§E2). *Unblocks "install → live" end-to-end with no engine change.*
- **4d-a-1 — SQL inline tools.** Parse tool bodies (§C1) + compile SQL tools (§C2) + register into the member toolset (§C3) + flip the validate message. Canonical SQLite-native demo pack (risk #5).
- **4d-a-2 — template inline tools.** Only if F-Q2 = in: introduce the engine (risk #1) + the output-adapter path (§C2).
- **4d-b — reactive workers.** `PackAgentExecutor` (agent-turn-per-event) + the pack-event `TriggerSource` (§D1/§D2) for the F-Q4 event set; budget cap (risk #4). Per-agent `hotl`/`on_failure` is a stretch (§D4).

---

## G. References (grep-verified 2026-06-26, against `main` `e76b31b`)

- `crates/xiaoguai-core/src/pack_agents.rs:{25-26,48,60-64,78-84,97-99,166-181}` — `AgentDef`/`AgentTool` keep only name+kind; `classify`; reactive skip; inline-tool warning.
- `crates/xiaoguai-core/src/pack_runtime.rs:{60,90-95,113-120,144,178-194,246-248,295-298,329-335,371-383,456,496-525,572-596}` — Phase-2 executors + SELECT guards; `scan_enabled_pack_agents`; `tool_allowlist: None`; `record_activated_agents`; "call once per process".
- `crates/xiaoguai-core/src/lib.rs:{548-580,587-633,666-695,717,943-960}` — shared anomaly registry + dedup as **locals**; Phase-2 boot-scans; webhook/file-watch event sources; job upserter; Phase-4b agent scan.
- `crates/xiaoguai-api/src/state.rs:{200,302,307,271}` — `job_upserter`/`personas`/`teams`/`skill_packs` on `AppState`; **no** anomaly-registry/dedup field.
- `crates/xiaoguai-api/src/orchestrate.rs:{108,186,256-264}` — member `Arc<Toolbox>`, `default_model`, `subset_toolbox`/`filter_tools`.
- `crates/xiaoguai-agent/src/toolbox.rs:42` — `Toolbox`.
- `crates/xiaoguai-scheduler/src/{trigger_source.rs:95,116, runner.rs:154/166/186 (with_budget_ledger/tick/fire_event), sqlite_repository.rs:35-104, trigger.rs:207, composite_executor.rs:47}` — event channel + `TriggerSource`; repo-driven tick; SQLite job upsert/list_due; `is_scheduled`; `CompositeExecutor::register`.
- `crates/xiaoguai-anomaly/src/registry.rs:{108,118,135-138,160}` — `register`/`deregister`/observe-unknown-warns/`registered_ids`.
- `crates/xiaoguai-core/src/scheduler_bridge.rs:435` — `spawn_file_watch_source` (template for a pack-event `TriggerSource`).
- `crates/xiaoguai-cli/src/commands/{pack.rs:98-130,367-373, remote.rs:24}` — out-of-process install (SQLite write, no serve call); `--server` resolver to reuse for the live poke.
- Parent design: `docs/plans/2026-06-25-skill-pack-loader-phase4.md` (§B5 agent-richer-than-persona, §C3/§C4 deferrals this doc opens, §D UX, §G open questions).
