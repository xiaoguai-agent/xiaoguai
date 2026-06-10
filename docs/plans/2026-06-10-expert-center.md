# Implementation plan — T3: Expert center productization

| | |
|---|---|
| Date | 2026-06-10 |
| Status | **APPROVED 2026-06-10** — owner resolved §5: ① tenant_id cleanup IN scope · ② suggest = deterministic A · ③ packs = display-only tags A |
| Parent | `docs/plans/2026-06-09-capability-upgrade.md` §2-C / §3-T3 |
| Hard constraints | DEC-033 unchanged: 单二进制 · 内嵌 SQLite · 单 owner · `:7600` |

## 0. Goal & scope

Turn the existing **personas + packs + orchestrator registry** parts into a
product surface: **pick an expert → (optionally) form an expert team → one-click
run**, with a selection panel in chat-ui and team management in admin-ui.

**Explicit boundary vs T4:** T3 ships expert/team *selection, composition and
storage* + intent-based expert *suggestion*. Actual **parallel multi-persona
execution + synthesis is T4** — a T3 "team session" runs with the team's lead
persona until T4 lands. This keeps T3 at size M and avoids pre-empting the
T4/T5/T6 orchestration DEC question (capability-upgrade §6.3).

## 1. What exists today (verified in code)

- **Personas — fully built** (`crates/xiaoguai-personas`): `Persona { id, name,
  system_prompt, default_model, tool_allowlist, escalation_tier, created_at,
  archived }` (`src/model.rs:14`); SQLite repo with `personas` +
  `session_personas` (one persona per session, DB-enforced); enforcement helpers
  (`tool_allowed` / `filter_tools` / `build_system_messages`) consumed by the
  ReAct loop. API: full CRUD at `/v1/personas` + attach/detach at
  `/v1/sessions/{id}/persona` (`crates/xiaoguai-api/src/routes/personas.rs`).
- **Admin UI persona pane exists** (`frontend/admin-ui/src/panes/Personas.tsx`):
  CRUD + role chip inferred from a `role/(planner|worker|critic)` token in the
  system prompt. ⚠️ Pane + shared client still carry **pre-pivot `tenant_id`
  plumbing** (`frontend/shared/src/index.ts:2020` sends `?tenant_id=`; backend
  `personas.rs` has no tenant concept and ignores it).
- **Packs — catalog + install API exist** (`/v1/skills/catalog|installed|install`,
  `crates/xiaoguai-api/src/skills.rs`); runtime loader integration still pending.
- **Orchestrator registry + capability router — built, unwired**
  (`crates/xiaoguai-orchestrator/src/registry/`): `AgentSpec` with
  `(domain, action)` capabilities, `CapabilityRouter::route(Intent) → Dispatch`
  (primary + fallbacks, ranked by cost_hint). No HTTP entry point exists.
- **Governance**: every tool call already passes
  `HotlGate::check(scope=tool_call.{name})` in
  `crates/xiaoguai-agent/src/react.rs` and persona `tool_allowlist` filtering —
  experts/teams inherit HotL + audit for free; nothing new needed there.
- **chat-ui has no persona surface at all**: session creation
  (`frontend/chat-ui/src/ChatPage.tsx` / `SessionList.tsx`) cannot pick a
  persona even though the backend attach API exists.

## 2. Design

### 2.1 Data model (new crate module in `xiaoguai-personas`, no new crate)

```text
expert_teams(
  id TEXT PK, name TEXT UNIQUE(active), description TEXT,
  lead_persona_id TEXT NOT NULL,            -- runs the session until T4
  member_persona_ids TEXT NOT NULL,          -- JSON array, ordered
  recommended_pack_slugs TEXT,               -- JSON array, optional
  created_at TIMESTAMP, archived BOOL
)
session_teams(session_id TEXT PK, team_id TEXT FK, attached_at TIMESTAMP)
```

- Immutable DTOs + `TeamRepository` trait + `SqliteTeamRepository` +
  `InMemoryTeamRepository`, mirroring the persona repo pattern 1:1.
- Validation at the boundary: lead must be a member; members must be active
  personas; ≥1 member.
- Attaching a team to a session **also attaches the lead persona** via the
  existing `session_personas` path, so the ReAct loop needs **zero changes**.

### 2.2 API (extend `xiaoguai-api`, same 503-when-absent pattern)

- `GET/POST /v1/teams`, `GET/PATCH/DELETE /v1/teams/{id}` — CRUD (archive on delete).
- `GET/PUT/DELETE /v1/sessions/{id}/team` — attach/detach (PUT also sets lead persona).
- `POST /v1/experts/suggest` — body `{goal}`; deterministic keyword-overlap
  scorer over active personas + teams (ASCII words + CJK bigrams; name matches
  weigh 2×), returns ranked suggestions. **Suggestion only — user confirms; no
  auto-attach.** *(Implementation note: deliberately does NOT go through
  `CapabilityRouter` — its exact AND-coverage semantics fit explicit-capability
  intents (T4), not fuzzy free text. See `routes/experts.rs` header.)*
- Audit: `team.create|update|archive`, `team.attach`, `expert.suggest` entries
  via the existing audit sink pattern.

### 2.3 chat-ui — expert picker

- Session creation gains an **expert picker** (persona or team;搜索 + role chip,
  reusing admin-ui's role inference); selection calls the existing
  `PUT /v1/sessions/{id}/persona` or the new `/team`.
- Active expert shown as a chip in the chat header; click to switch (re-attach).
- "一句话找专家": optional input → `/v1/experts/suggest` → confirm card.

### 2.4 admin-ui — team management

- New pane `ExpertTeams.tsx`: CRUD with persona multi-select, lead picker,
  recommended-pack tags. Follows the `Personas.tsx` drawer/table pattern and
  LLD-ADMIN-UI-001 empty/loading/error states.
- **In-scope cleanup**: strip the dead `tenant_id` plumbing from
  `listPersonas`/`Personas.tsx` while touching these exact surfaces
  (owner to confirm; it is pre-pivot leftover, backend ignores it).

## 3. Task breakdown (each = TDD, clippy `-D warnings` + nextest green)

| # | Task | Size | Verification point |
|---|---|---|---|
| T3.1 | Team model + repos + migration | S | unit tests: validation, archive, attach-sets-lead |
| T3.2 | `/v1/teams` + `/v1/sessions/{id}/team` routes + audit entries | S | route tests incl. 503-when-absent, audit assert |
| T3.3 | `/v1/experts/suggest` (persona→AgentSpec mapping + router wiring) | M | golden tests: goal → ranked suggestions; empty-match error |
| T3.4 | shared client: team + suggest methods; remove tenant_id leftover | S | type parity tests; chat-ui/admin-ui compile |
| T3.5 | chat-ui expert picker + header chip + suggest card | M | Playwright spec: pick expert → session uses persona prompt |
| T3.6 | admin-ui ExpertTeams pane | M | Playwright spec: create team → appears in chat picker |
| T3.7 | docs: user guide + handoff update | S | docs build |

Sequencing: T3.1 → T3.2 → (T3.3 ∥ T3.4) → (T3.5 ∥ T3.6) → T3.7.
Clean-box boot (`serve` on fresh SQLite, `:7600`, `/healthz`) stays green.

## 4. Boundaries

- No parallel multi-persona execution / synthesis (T4), no consult-execute mode
  flag (T5), no self-healing wiring (T6).
- No new crate, no schema changes to `personas`/`session_personas`.
- No tenant/team-permission model — an "expert team" is a composition object,
  not an access-control object (DEC-033 single owner intact).
- Suggest endpoint is read-only and offline (no LLM call in MVP — capability
  matching is deterministic; an LLM-ranked upgrade can come with T4).

## 5. Open questions — RESOLVED by owner 2026-06-10 (① yes ② A ③ A)

1. **tenant_id cleanup in scope?** (§2.4 — recommended yes, it's dead pre-pivot
   plumbing on exactly the files T3 touches.)
2. **Suggest matching source**: role token + keywords (deterministic, offline,
   MVP) vs adding an explicit `capabilities` field to `Persona` (schema change,
   cleaner long-term). Plan assumes MVP first, field can come later.
3. **Pack linkage depth**: MVP stores `recommended_pack_slugs` as display-only
   tags (install stays in admin-ui). One-click "install team's packs" can follow
   once the pack runtime loader lands.
