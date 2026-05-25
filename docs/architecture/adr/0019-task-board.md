# ADR-0019 — Durable Task Board (Kanban-style multi-agent work queue)

Date: 2026-05-25
Status: Proposed (v1.4 target)

## Context

Xiaoguai supports multi-agent parallelism (ADR-0006 MCP Tasks, v1.1.5a peer topology)
but the work-distribution layer is entirely human-operated. During the 2026-05-25 design
session, the team ran a pool of ten sub-agents manually — pasting tasks, polling for
completion, reassigning blocked cards by hand. This worked, but the bottleneck was
obvious: **human orchestration saturated faster than agent capacity**.

Hermes Desktop's Kanban view surfaced the right mental model: a first-class board where
agents autonomously pick up and finish tasks. Columns mirror the full lifecycle —
TRIAGE / TO-DO / READY / RUNNING / BLOCKED / DONE — and operators can tune dispatch
policy and pool size without touching code.

Two pressure points make this a v1.4 priority rather than a later add-on:

1. **Pack integration**: devops-oncall and incident-triage packs already fire events
   (PagerDuty alerts, GitHub notifications) that need to become durable cards, not
   ephemeral chat messages. Without a board, every event is a race against context
   eviction.

2. **Outcome attribution**: the outcome-telemetry subsystem (merged in wave-3) records
   per-session events but has no first-class handle for multi-step autonomous work. A
   card's lifecycle log IS the attribution chain — it solves the "which agent actually
   resolved this alert?" traceability gap without bespoke instrumentation.

### Non-goals

- Replacing the scheduler crate (cron-triggered jobs remain separate).
- Becoming a general project-management tool (no sub-tasks, no dependencies between
  cards beyond what the agent writes in the card body).

## Decision

Implement a **Tasks subsystem** as a new first-class feature of Xiaoguai:

### Card model

Each card carries:

```
id          UUID v7 (time-ordered for efficient range scans)
board_id    FK → boards table (multi-board per workspace)
title       text, operator-visible
body        text, agent-editable (markdown; the agent's working scratchpad)
column      TRIAGE | TO-DO | READY | RUNNING | BLOCKED | DONE
priority    0–255 (higher = more urgent; used by priority-weighted dispatcher)
affinity    optional agent_id or tag (e.g. "python", "infra")
created_by  user_id or pack_name
assigned_to nullable agent_id (set by dispatcher on RUNNING transition)
blocked_reason  nullable text (populated on BLOCKED transition)
```

Card transitions are append-only events in a `card_events` table:
`(card_id, from_col, to_col, actor, reason, ts)`. This is the attribution chain
fed into outcome-telemetry.

### Column semantics

| Column | Meaning | Who transitions in | Who transitions out |
|--------|---------|-------------------|---------------------|
| TRIAGE | Arrived from a pack or via REST, needs human review | Pack / REST / IM | Human or auto-triage rule |
| TO-DO | Human-approved, not yet ready to dispatch | Human | Human (marks READY) or HotL approval |
| READY | Available for the dispatcher | Human / HotL-approved | Dispatcher |
| RUNNING | Assigned to an agent | Dispatcher | Agent (success → DONE, blocked → BLOCKED) |
| BLOCKED | Agent needs human input or an external event | Agent | Human or external webhook |
| DONE | Terminal. Immutable after 5-minute grace period. | Agent / Human | — |

### Dispatch policy (configurable per board)

Three strategies, set in board configuration:

- `fifo` — first-in-first-out within READY; default.
- `priority` — highest `priority` field wins; ties broken by insertion order.
- `round-robin` — distributes evenly across available agents (useful when agents
  specialise by affinity tag and you want balanced load, not pure priority).

The dispatcher runs as a tokio task inside `xiaoguai-core`. On each tick (default 2s,
configurable) it:

1. Queries READY cards up to `pool_size - len(RUNNING cards)` slots.
2. Applies affinity filter (if card has affinity tag, only consider agents that match).
3. Moves selected cards to RUNNING, sets `assigned_to`.
4. Emits `task.dispatched` outcome event (attribution starts here).

`pool_size` defaults to 5; operators set it per board. Rule of thumb: do not exceed
half the PG `max_connections` pool reserved for `xiaoguai-core`.

### Multi-board layout

Boards are scoped to a tenant (multi-tenant RLS already established in v1.0). Common
patterns operators use:

- One board per team (platform, security, devops).
- One board per pack (incident-triage, hr-onboarding).
- One board per environment (prod-oncall, staging-experiments).

The REST API exposes `/v1/boards` and `/v1/boards/{id}/cards` with standard CRUD plus
`POST /v1/boards/{id}/cards/{cid}/transition` for explicit column moves.

### Integration points

**Packs**: a pack can call `board.create_card(title, body, column="TRIAGE")` via the
MCP `task_create` tool. devops-oncall auto-creates a TRIAGE card on each PagerDuty
alert; incident-triage does the same for Grafana alerts. The operator decides whether
cards flow automatically to TO-DO or wait for human review.

**HotL (Human-on-the-Loop)**: when an agent determines it needs human approval to
proceed, it transitions the card to BLOCKED and populates `blocked_reason`. The HotL
subsystem surfaces this as an approval request. On approval, the card returns to READY
for re-dispatch.

**Outcome telemetry**: every column transition records an outcome event. The full
lifecycle of a card — TRIAGE → TO-DO → READY → RUNNING → BLOCKED → RUNNING → DONE —
becomes a queryable attribution chain, answering "who did what, when, and how long
did each stage take?".

### New crate: `xiaoguai-tasks`

Responsibility boundary:

- `Board`, `Card`, `CardEvent` domain types + Postgres-backed repository.
- Dispatcher loop (tokio task).
- REST handlers (registered into `xiaoguai-core` AppState under `task_board`).
- Admin UI page: board view with column swimlanes, Refresh button, Dispatch-now button.
- Migration: one new SQL migration file (follows `0015_skill_packs.sql` numbering).

The crate does **not** own agent lifecycle — it calls into the existing agent registry
(feat/agent-registry) to enumerate available agents and their affinity tags.

## Alternatives considered

### Use the existing scheduler crate for queuing

Rejected. The scheduler crate is cron-based (time-triggered, idempotent jobs). It has
no concept of a human-visible card, no column state, no blocking/approval workflow, and
no assignment semantics. Layering a queue on top would mean two separate mental models
for operators ("cron" vs. "task") with overlapping but non-identical behaviour.

### External integration (Jira / Linear / Asana)

Rejected as the primary path. External systems introduce latency (API round-trips),
cost (per-seat pricing), and break air-gap deployments. They also scatter attribution
data across two systems. A first-class internal board keeps telemetry co-located with
the agent logs.

An **export adapter** (push completed cards to Linear) is deferred to v1.5 as a pack.
It is not part of this ADR.

### Pure CLI-only (`xg tasks` subcommand, no board UI)

Included as a complement, not a replacement. `xg tasks list`, `xg tasks move`, and
`xg tasks dispatch` are required for operators working in terminal-only environments
(air-gap, SSH). The admin UI board view is the primary operator surface.

## Consequences

### Positive

- Autonomous multi-agent work becomes first-class: operators configure pool size and
  dispatch policy; no human orchestration required per-task.
- Full audit trail: every card state transition is a timestamped, actor-attributed event
  in Postgres, queryable via the existing outcome-telemetry pipeline.
- Operators can visualise agent workload and bottlenecks (RUNNING queue depth,
  BLOCKED queue depth, mean time per stage).
- Packs gain a durable event-to-work handoff that survives restarts and context eviction.

### Negative / Costs

- New subsystem: one new crate (`xiaoguai-tasks`), one new migration, one new admin UI
  page, new REST endpoints. Adds ~800–1200 LOC to the workspace.
- Pack coordination required: each pack that auto-creates cards must declare which board
  it targets and under what conditions (alert severity threshold, time-of-day, etc.).
  This is pack-specific configuration that ships per-pack.
- Multi-tenant scoping at board level adds a RLS policy. Pattern is already established
  (v1.0 auth), but each new table requires explicit policy.
- Dispatcher tick is a background tokio task. Under high card volume, the 2s default
  tick introduces 0–2s dispatch latency. Operators on latency-sensitive workflows
  should lower the tick interval, accepting higher PG query rate.

## Open questions

1. **Workspace scoping**: Is "board per tenant" sufficient, or do we need a
   sub-tenant "workspace" concept (e.g., per-team within one tenant)? If yes, this
   adds a `workspaces` table and complicates RLS. Deferred decision: ship tenant-scoped
   boards in v1.4; evaluate workspace demand from operator feedback.

2. **Auto-promotion TRIAGE → TO-DO**: When should a pack-created card auto-promote
   without human review? Proposed: boards have an `auto_triage` flag; when set, cards
   from trusted packs (whitelist) skip TRIAGE and land directly in TO-DO. Default: off.

3. **Card expiry / archival**: DONE cards accumulate. Do they roll into an archive table
   after N days, or stay in the main `cards` table with a soft-delete flag? PG
   partition-by-created-at is the likely answer; not decided.

4. **Dispatcher fairness with affinity**: if all READY cards carry affinity `"infra"` but
   only one `"infra"` agent is available, other agents idle. Should the dispatcher fall
   back to unaffined agents after a configurable wait? Not decided.

## Cross-references

- ADR-0006: MCP Tasks primitive — async tool call semantics that cards build on.
- ADR-0008: Tool result provenance — card events extend this provenance chain.
- ADR-0013: Zero-default telemetry — card lifecycle events follow the same opt-in model.
- ROADMAP v1.4 candidates section (this ADR IS that entry).
- Glossary: "Workspace" (see open question 1 above — concept is not yet formalised).
- Operator guide: `docs/book/src/operator/task-board.md` (this ADR's companion chapter).
