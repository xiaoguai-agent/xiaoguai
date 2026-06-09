# Expert Center — pick an expert, form a team, one-click run

T3 of the capability-upgrade plan (`docs/plans/2026-06-10-expert-center.md`).
The expert center turns personas and persona *teams* into a product surface:
pick an expert in chat, compose teams in the admin UI, or describe your goal
in one sentence and let xiaoguai suggest who should handle it — fully offline,
deterministic, no LLM call involved in the suggestion.

> **Team execution model (until T4 lands):** a team session runs with the
> team's **lead persona**. Attaching a team also attaches its lead via the
> normal session-persona path, so HotL gating, tool allowlists, and audit all
> behave exactly as for a single persona. Parallel multi-persona orchestration
> is T4.

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
