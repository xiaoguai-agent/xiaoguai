# Skill Pack Runtime Loader — Phase 4 Technical Design (agent-team execution + UX)

**Status:** DESIGN — review checkpoint (2026-06-25). No Phase-4 code is written. Owner requested this doc ("好的，写") and added the scope cut **"互动界面友好化 + 新功能介绍 + 如何上手 + 复杂任务对接"** — so this design covers *both* the execution engine **and** the chat-UI experience.

**Context.** The skill-pack loader has shipped Phase 1 (`pack validate`, #336/#338/#344) and Phase 2 (watch/anomaly execution, v1.26.0, #352/#353). Phase 3 (per-pack SQL migrations) stays deferred — the [corpus disposition](2026-06-24-skill-pack-corpus-disposition.md) found the shipped packs target operator-owned schemas, so they are **templates**, not runnable-here. Decision #4 (agent execution) was explicitly **out of scope for loader v1** in the [Phase 2 design](2026-06-23-skill-pack-loader-phase2.md §A.4) and the [Phase 0 record](2026-06-21-skill-pack-loader.md §2.4). This doc opens it.

> Methodology (per the repo's "verify before citing in design docs" rule): every code fact below was grep-checked on 2026-06-25 and is labelled **V**. Recommendations are **REC**. §B5 records where a scouting pass **over-simplified** — do not propagate the simpler story.

---

## A. The question Phase 4 answers

A pack's `agents[]` declares an **agent team** (e.g. the `vmware-ops` expert team, `sales-qualification`'s BANT/MEDDIC agents). Today installing such a pack records a row and nothing more — `activation_status` is hard-coded `"pending"` and the team never comes alive. Phase 4 = **make an installed pack's agent team usable for a complex task, and make that obvious in the UI.**

---

## B. Verified runtime surface (grep-checked 2026-06-25)

### B1. The test-only registry — **V**
- `AgentRegistry` (`crates/xiaoguai-orchestrator/src/registry/mod.rs:177`) + `CapabilityRouter` (`registry/router.rs:58`). All four constructors (`mod.rs:336,372`, `router.rs:157,188`) sit in `#[cfg(test)]` modules; **no `xiaoguai-api`/`-runtime`/`-core` reference exists**. CONFIRMED test-only. They are a *capability-routing* abstraction (`Intent{domain,action}` → agent), **not** the team-execution path the product actually ships (§B2) — a fork in the road, see §C.

### B2. The team-execution engine that **already ships** — **V**
- `POST /v1/sessions/{id}/orchestrate` (`routes/mod.rs:280` → `routes/orchestrate.rs:152 orchestrate_session`) is the live, v1.20+ path. It resolves a **Team**, builds an `OrchestrateMemberRunner` (`orchestrate.rs:106`, `impl MemberRunner`), and drives an `ExecutiveRunner<R>` (`patterns/executive.rs:185`; trait `MemberRunner` at `:106`) that **fans members out in parallel and has the lead persona synthesize** the result. SSE-streamed, audit-chained, HotL-gated — all already wired.
- **The members are `Persona`s.** This is the reuse target.

### B3. Persona / Team model — **V**
- `Persona` (`crates/xiaoguai-personas/src/model.rs:14`): `{ id, name, system_prompt, default_model, tool_allowlist, escalation_tier, archived }`.
- `Team` (`personas/src/teams/model.rs:18`): `{ id, name, description, lead_persona_id, member_persona_ids, glossary_md, recommended_pack_slugs }`. `recommended_pack_slugs` is **display-only today — no execution link**.
- `AppState` (`crates/xiaoguai-api/src/state.rs`) holds `personas` (`:302`) + `teams` (`:307`) repos and `skill_packs` (`:271`). **No `agent_registry` field.**

### B4. Pack `agents[]` is a list of file refs — **V**
- `PackManifest.agents: Vec<PackPath>` (`crates/xiaoguai-core/src/packs.rs:86`); `PackPath{path}` (`:135`). `PackLoader::load` does **path-only** validation; **nothing parses an agent YAML into a struct.** There is no Rust `AgentDef` type yet.

### B5. ⚠️ A pack agent is **richer than a Persona** (the scout's over-simplification) — **V**
A real agent YAML — `packs/events-management/agents/registration-triage.yaml` — carries far more than a persona:
- `name`, `kind: worker`;
- **`triggers[]`** — event subscriptions (`event: events.vip_registration_needs_handling`, `kind: inbound`);
- `llm: { model, temperature, max_tokens }`;
- `system_prompt` (multi-line);
- **`tools[]`** — *inline tool definitions* (`kind: sql` + a `query`, or `kind: template` + a `.j2` + an `output:` adapter), **not** references to platform tools;
- `hotl: { autonomous_actions, notify_channel }` — per-agent governance;
- `on_failure: { retry, fallback_status, notify_channel }`.

A `Persona` is only `(system_prompt, default_model, tool_allowlist)`. So **"convert agent → persona" is lossy**: it drops triggers, the inline tool *bodies*, the per-agent HotL, and on_failure. Any design that pretends agent≡persona is wrong.

### B6. There are **two kinds** of pack agent — **V**
- **Conversational / orchestration members** — no `triggers`; a role + prompt (+ maybe tools). These map onto the §B2 team path (`vmware-ops`, `sales-qualification`, the "专家团队" packs).
- **Reactive workers** — have `triggers[]` (event/inbound subscriptions, like `registration-triage`). These are *event-driven*, closer to the Phase-2 watch runtime than to a session-scoped team member. **Different execution model.**

### B7. Install + the activation seam — **V**
- `InstalledSkillPackResponse.agents: Vec<String>` is hard-coded empty (`skills.rs:159`); `activation_status` is the literal `"pending"` (`:180`). Phase 2 added `enabled`+`pack_dir` to the `installed_skill_packs.config` JSON and a serve **boot-scan** (`core/lib.rs:587 scan_enabled_pack_anomalies`, `:612` watches; `pack_runtime.rs:337 enabled_pack_dirs`). Phase 4 follows the same shape.

### B8. The complex-task UI is **already built** — **V**
- `ExpertPicker` (`frontend/chat-ui/src/ExpertPicker.tsx:46`) attaches a team to the session; `ChatPage.runTeam()` (`ChatPage.tsx:462`) calls `client.orchestrateSession(sid, {goal, team_id}, …)` (`shared/src/index.ts:2588`) and **renders live member progress + the lead's synthesis** from the orchestrate SSE stream. `PaneIntro` (`admin-ui/src/components/PaneIntro.tsx`, used at `SkillPacks.tsx:494`) is the in-UI "what is this / how to use it" intro the owner likes.
- **So the complex-task → team pipe exists.** What's missing is that **pack-installed teams never appear in the picker** (because `agents[]` never become personas/teams), plus the intro/onboarding glue.

---

## C. Key design resolution — execution engine

> **v1 = reuse the orchestrate path. On boot, derive Personas + a Team from a pack's *conversational* agents and upsert them; the existing `/orchestrate` engine then runs the pack's team unchanged.** Do **not** build the test-only `AgentRegistry` into serve, and do **not** build a second execution lifecycle.

Rationale — same reuse-over-build discipline as Phase 2 ([[feedback-reuse-over-build]], DEC-033): `ExecutiveRunner` + `OrchestrateMemberRunner` already do parallel fan-out, lead synthesis, SSE, audit, and HotL. The capability-router fork (§B1) is a *different* abstraction and would duplicate all of it.

### C1. Conversational agents → Personas + a Team (REC)
For each pack agent **without `triggers`**: map `name → name` (namespaced `pack-slug/agent-name`), `system_prompt → system_prompt`, `llm.model → default_model`, and the *names* of any `tools[]` that resolve to platform tools `→ tool_allowlist` (un-resolvable inline tools are dropped with a `pack validate` warning). Upsert a `Persona` per agent (managed/archived-aware so they don't clutter operator CRUD). Then upsert one `Team`: lead = the pack's declared lead (see §G2), members = the rest, `description` = the pack description + agent roles (feeds `pick_team_for_goal` routing + the glossary). Tag it with the pack slug.

### C2. The lossy mapping is explicit, not hidden (REC)
v1 runs the **conversational layer only**: the agents reason with their `system_prompt` and the **platform toolbox**, not their bespoke pack `tools[]`. Dropped in v1: inline tool bodies (§C4), `triggers` (§C3), per-agent `hotl`/`on_failure` (the platform's global HotL + audit still apply). `pack validate` (Phase 1) is extended to **report** exactly what will and won't activate, so an author is never surprised. This mirrors the v1.27 catalog "template" honesty — the UI says so too (§D1).

### C3. Reactive workers (`triggers[]`) — deferred to Phase 4b
Event-triggered agents are not session-orchestration members; running them = binding pack agents to the event/watch bus (Phase-2 territory) + an agent loop per event. Bigger, separate. v1 **validates** them and **excludes** them from the derived team (with a surfaced note), rather than mis-running them.

### C4. Inline pack tools (`tools[]` bodies) — deferred to Phase 4b (the heaviest piece)
Compiling each inline `sql`/`template` tool into an agent-callable (parameterized SQL against the read pool, template-render → output adapter) is the largest build and re-treads §B8's corpus reality (the shipped tools are Postgres/foreign-schema). Its own design doc. v1 uses the platform toolbox.

### C5. AppState + boot-scan (REC)
No new registry. Add `scan_enabled_pack_agents(pool, persona_repo, team_repo)` in `run_serve` alongside the Phase-2 scans: for each enabled pack, parse `pack.yaml` + its conversational agent YAMLs, upsert personas + the team. **Idempotent** (keyed by pack slug + agent name). Populate the now-empty `InstalledSkillPackResponse.agents` (`skills.rs:159`) and **flip `activation_status` → `"active"`** once the team is registered — finally closing the Phase-5 marketplace↔loader seam for agent packs.

### C6. Identity / audit / HotL — reuse (V)
The orchestrate path is already audit-chained and HotL-gated through the runtime. No new governance surface; single-owner (DEC-033) means no per-tenant team scoping.

---

## D. Interactive surface — friendly-ization, intro, onboarding, complex-task hand-off

The owner's explicit addition. None of this is backend plumbing; it is the experience that makes the feature *land*.

### D1. New-feature introduction (the PaneIntro pattern) — REC
- **Skills page (chat-ui + admin-ui):** under the "专用场景" tab, a short bilingual intro (reusing `PaneIntro` / the v1.27 disclaimer style): *"专用场景的技能包自带一个 agent 专家团队。安装后，团队会出现在聊天的「专家」选择器里，你可以把一个复杂任务交给整个团队——成员并行处理、组长汇总成一个答案。"* + a one-line honest scope note: *"v1 为对话型团队；包内自定义工具/事件触发将在后续版本接入。"*
- **ExpertPicker:** a first-run tooltip/empty-state explaining what a team is and that installing a 专用场景 pack adds one.

### D2. 如何上手 (onboarding flow) — REC
A concrete, three-step path, surfaced inline so the owner never reads a manual:
1. **Skills 页** → 装一个带团队的包 → 卡片立刻显示 **「团队已激活 ✓」** 徽章 + 一个 **「去聊天使用 →」** 链接（深链到 `/` 并预选该团队）。
2. **聊天页** → `ExpertPicker` 顶部高亮新激活的团队 → 点选挂到当前会话。
3. 输入一个复杂目标 → 点 **「交给团队」**（`runTeam`）→ 实时看到每个成员的进度卡片 + 组长的最终汇总。
- Backend support needed: the install response carries `agents[]` + `activation_status:"active"` (§C5) so the Skills card can render the badge + deep-link; the ExpertPicker reads the same teams repo, so a pack team shows up with **zero new API**.

### D3. 复杂任务对接 (already built — make it discoverable) — V + REC
The hard part is done (§B8): `runTeam` → `orchestrateSession` → streamed parallel members + lead synthesis. Phase 4's job is **discoverability + framing**, not new pipes:
- pack teams appear in `ExpertPicker` (via §C1's upserted Team) — the single missing link;
- the orchestrate progress UI gets pack-aware labels (member = `pack-slug/agent-name`, so the user sees "vmware-ops/vsan-expert is analyzing…");
- a "建议目标" affordance: seed the goal box with example complex tasks from the pack description, so the owner sees *what* to ask a team (lowers the blank-page cost).

### D4. Bilingual, per the v1.27 standard — REC
Every new string (intro, badge, onboarding, picker labels) ships zh + en (+ ja chrome) at i18n parity, enforced by the existing `parity.test.ts` — consistent with the catalog work just released.

---

## E. Phase 4 — sliced into reviewable PRs (TDD, each gated on Build-and-test)

- **4a — parse + map (no serve change).** New `AgentDef` parse of agent YAML; pure `agent → Persona` / `pack → Team` mapping; classify conversational vs reactive (§B6); extend `pack validate` to report what will/won't activate (§C2). Table-tested. *Lowest risk; unblocks the rest.*
- **4b — boot-scan + activation.** `scan_enabled_pack_agents` in `run_serve`; upsert personas + team; populate `InstalledSkillPackResponse.agents`; flip `activation_status → active` (§C5). Tests: install pack → boot → team exists → existing `/orchestrate` runs it.
- **4c — the UX slice + a canonical runnable pack.** The §D intro / badge / deep-link / onboarding / pack-aware orchestrate labels (bilingual); **plus** a canonical agent pack authored against xiaoguai's own tables + platform toolbox (the `observability-starter` equivalent for agents) proving install → pick → complex task → real multi-agent answer end-to-end.
- **4d — Phase 4b (separate doc).** Inline pack-tool execution (§C4) + reactive/triggered workers (§C3). The heavy lift; gated on its own design.

---

## F. DEC-033 guardrails + explicit scope cuts

- **Binding:** personas/teams already live in the one SQLite — no new store, no per-tenant scoping. Reuse the orchestrate lifecycle — no second execution daemon. Packs ship in-repo / operator dirs, not a network registry.
- **Cut from v1:** inline pack tools (§C4); reactive/triggered workers (§C3); the test-only `AgentRegistry`/`CapabilityRouter` (superseded by the orchestrate path — left as-is, flagged as candidate cleanup in §G4); per-pack migrations (Phase 3); per-agent `hotl`/`on_failure` from the YAML (platform HotL applies).

---

## G. Risks / open questions for review

1. **Lossy activation expectation.** Authors expect their `tools[]`/`triggers` to run; v1 runs only the conversational layer. Must be loud — `pack validate` report + the §D1 UI scope note. Risk: "I installed it but my agent's SQL tool didn't fire."
2. **Team composition — who leads?** Agent YAMLs carry no `lead` flag. **REC:** add an optional `lead_agent:` to `pack.yaml`; else first agent, or synthesize a lightweight coordinator persona. Spike in 4a.
3. **Persona namespace collisions** between pack agents and operator personas. **REC:** prefix pack personas with the slug + a `managed`/`archived` flag so they stay out of the operator's persona CRUD.
4. **The vestigial capability router.** If Phase 4 takes the orchestrate route, `AgentRegistry`/`CapabilityRouter` stay dead. **REC:** leave for now; note as a candidate deletion (don't expand scope to remove it here).
5. **Corpus.** The shipped pack agent YAMLs are Postgres/foreign-schema templates (§B5/§B8); 4c **must** author a SQLite-native canonical pack to demo — exactly as Phase 2 shipped `observability-starter`.

---

## H. References (grep-verified 2026-06-25)

- `crates/xiaoguai-orchestrator/src/registry/{mod.rs:177,336,372, router.rs:58,157,188}` — `AgentRegistry`/`CapabilityRouter`, all constructors `#[cfg(test)]`.
- `crates/xiaoguai-orchestrator/src/patterns/executive.rs:{106,185}` — `MemberRunner` trait, `ExecutiveRunner`.
- `crates/xiaoguai-api/src/{routes/mod.rs:280, routes/orchestrate.rs:152, orchestrate.rs:106}` — orchestrate route + `OrchestrateMemberRunner`.
- `crates/xiaoguai-personas/src/{model.rs:14, teams/model.rs:18}` — `Persona`, `Team`.
- `crates/xiaoguai-api/src/state.rs:{271,302,307,325}` — `skill_packs`/`personas`/`teams`/`watchers`; no agent registry.
- `crates/xiaoguai-core/src/packs.rs:{86,135}` — `PackManifest.agents: Vec<PackPath>`, `PackPath`.
- `packs/events-management/agents/registration-triage.yaml` — real agent shape (triggers/llm/system_prompt/tools/hotl/on_failure).
- `crates/xiaoguai-api/src/skills.rs:{159,180}` — empty `agents`, `activation_status:"pending"`.
- `crates/xiaoguai-core/src/lib.rs:{587,612}`, `pack_runtime.rs:{337}` — Phase-2 boot-scan pattern to mirror.
- `frontend/chat-ui/src/{ExpertPicker.tsx:46, ChatPage.tsx:462}`, `frontend/shared/src/index.ts:2588`, `frontend/admin-ui/src/components/PaneIntro.tsx` — the existing complex-task UI + intro pattern.
