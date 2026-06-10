# Expert Center — pick an expert, form a team, one-click run

T3 of the capability-upgrade plan (`docs/plans/2026-06-10-expert-center.md`).
The expert center turns personas and persona *teams* into a product surface:
pick an expert in chat, compose teams in the admin UI, or describe your goal
in one sentence and let xiaoguai suggest who should handle it — fully offline,
deterministic, no LLM call involved in the suggestion.

> **Team execution model:** a team session runs with the team's **lead
> persona**. Attaching a team also attaches its lead via the normal
> session-persona path, so HotL gating, tool allowlists, and audit all
> behave exactly as for a single persona. For parallel multi-persona
> execution of a single goal, use the orchestrate endpoint (§6, T4).

---

## 1. Concepts

| Concept | What it is |
|---|---|
| **Persona** (expert) | A named role profile: system prompt + optional model override + tool allowlist + HOTL escalation tier. |
| **Team** | A named composition of personas: ordered member list with a designated **lead** (must be a member). Optionally tagged with recommended pack slugs (display-only). |
| **Suggestion** | A deterministic ranking of personas + teams against a free-text goal (keyword overlap; name matches weigh double; Chinese matched via character bigrams). |

Single owner (DEC-033): teams are composition objects, **not** an access-control
or tenant concept.

## 2. Picking an expert in chat

In the chat header, the **expert chip** shows the session's active expert
(team name takes display precedence over a bare persona). Click it to open the
picker:

- **Browse**: filterable lists of personas and teams; click to attach.
- **一句话找专家**: type a goal (e.g. *"帮我分析季度财报"* or *"review the
  release pipeline"*) and pick from the ranked suggestions.
- **Remove**: detaches the team and persona from the session.

Attaching a team immediately makes the session speak (and tool-gate) as the
team's lead persona. Switching experts mid-session takes effect on the next
turn.

## 3. Managing teams in the admin UI

The **Expert Teams** pane (`/expert-teams`) provides create/edit/archive:

- members: multi-select over active personas (role chips inferred from
  `role/planner|worker|critic` tokens in the system prompt);
- lead: restricted to selected members (save is blocked otherwise — the
  backend re-validates);
- recommended packs: comma-separated slugs, shown as tags at selection time
  (installation still happens in the Skill Packs pane).

Archiving a team hides it from listings and blocks new attachments; existing
sessions keep running.

## 4. API

| Method | Path | Notes |
|---|---|---|
| GET/POST | `/v1/teams` | list / create |
| GET/PATCH/DELETE | `/v1/teams/{id}` | fetch / partial update / archive |
| GET/PUT/DELETE | `/v1/sessions/{id}/team` | active team / attach (also attaches lead persona) / detach (lead persona stays) |
| POST | `/v1/experts/suggest` | `{"goal": "..."}` → ranked `suggestions` (read-only) |

All endpoints return 503 when the subsystem isn't wired (standard pattern).
Validation errors (empty members, lead not a member, unknown/archived member
persona) return 400; duplicate names 409.

## 5. Governance

Every team mutation and attachment writes a best-effort entry to the HMAC
audit chain: `team.create`, `team.update`, `team.archive`, `team.attach`
(with session and lead persona in the details). Suggestion calls are
read-only and not audited. Tool calls in a team session pass the same
`HotlGate::check` + persona `tool_allowlist` enforcement as ever — the team
layer adds no new execution surface.

## 6. API: orchestrated team runs (T4)

T4 of the capability-upgrade plan
(`docs/plans/2026-06-10-executive-orchestration.md`) gives teams a real
execution model: **goal in → members run in parallel → lead synthesizes one
answer out**, all inside a single session turn.

| Method | Path | Notes |
|---|---|---|
| POST | `/v1/sessions/{id}/orchestrate` | `{"goal": "...", "team_id"?: "...", "max_members"?: 8}` → SSE stream of run events |

Omit `team_id` to auto-route the goal to the top team from the suggest
scorer (§1) — 422 when nothing matches. `max_members` caps the team size
per request (engine default 8). The session must exist and be active (404 /
409 otherwise); 503 when personas or teams aren't wired.

**SSE events** (frame `event:` name = the `type` field; `data:` = the JSON
body; `id:` = a per-stream sequence number):

| Event | Payload | Meaning |
|---|---|---|
| `run_started` | `{members}` | run accepted; member fan-out begins |
| `member_started` | `{id}` | one member persona's agent turn started |
| `member_completed` | `{id, ok}` | a member finished (failures don't abort the run) |
| `synthesis_started` | `{ok_members}` | lead synthesis begins over the survivors |
| `final` | `{ok, text, failed_members}` | terminal event; `text` is the synthesized answer when `ok` |

Only the synthesized text is persisted — the goal as the user message, the
synthesis as the assistant reply. Member transcripts surface through the
SSE stream, attribution, and audit, not `messages`.

**Governance** — an orchestrated run is governed end-to-end:

- **HotL**: a turn-level `llm_call` check for `members + 1` calls up front
  (fail-closed, like a normal chat turn); per-tool gates apply inside each
  member run automatically.
- **Audit**: `orchestration.start` (team, member count, run id) and
  `orchestration.complete` (ok, failed members, run id) on the HMAC chain.
- **Attribution**: each member/lead turn stamps token usage as
  `orch:<run_id>:<persona_id>` — disjoint from `sess_*`, same synthetic-label
  convention as `scheduler:<job_id>` / `im:<provider>:<conv>`.
- **Turn lock**: the run holds the session's turn lock for its whole
  duration — a concurrent message or second orchestrate gets 409
  (`a turn is already in flight`).
- **Client-disconnect-safe**: the run is driven by a detached server task;
  it completes, persists, and audits even if the SSE client drops. The
  shared client (`orchestrateSession`) deliberately has no auto-reconnect —
  after a dropped stream, re-fetch the session messages to read the result.

Chat-ui surfacing (a team-run mode control) arrives with T5's
consult/execute split; T4 ships engine + API + shared client.
