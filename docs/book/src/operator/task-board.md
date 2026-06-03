# Task Board

> **Availability**: v1.4 and later. If you are running v1.3 or earlier there is no
> board. Use `xg tasks` CLI commands are also not available until v1.4.

The Task Board is a durable, multi-agent work queue that lives inside Xiaoguai. It
gives operators a Kanban-style view of every unit of work the agent pool is handling —
what just arrived, what is running, what is blocked waiting for your input, and what
is done. Agents pick up and finish tasks autonomously; you configure how many run in
parallel and how they are selected.

## What the board is

A **board** is a named, tenant-scoped queue of **cards**. Each card represents one
discrete unit of work: an alert to triage, a PR to review, an onboarding ticket to
process. Cards live in exactly one column at a time:

```
TRIAGE → TO-DO → READY → RUNNING → BLOCKED
                                      ↓
                              (human approves)
                                      ↓
                                   READY → RUNNING → DONE
```

You can have multiple boards — one per team, one per project, one per pack. All boards
are isolated by tenant (Postgres RLS).

The admin UI shows boards as swimlane columns with a card count badge per column. A
**Refresh** button reloads card state without page-refresh. A **Dispatch now** button
triggers the dispatcher immediately instead of waiting for the next tick.

## Card lifecycle

### TRIAGE

A card enters TRIAGE when created by a pack, a webhook, or the REST API. No agent
touches it here. This column is the intake buffer — it exists so operators can review
inbound work before committing it to the queue.

Transition out: a human moves it to TO-DO, or the board has `auto_triage = true` and
the card was created by a trusted pack (auto-promoted on arrival).

### TO-DO

Work has been acknowledged. It is not yet marked ready for dispatch — perhaps it needs
more context, a priority assignment, or an affinity tag. Agents do not see TO-DO cards.

Transition out: a human or an automated rule marks the card READY.

### READY

The card is in the dispatch queue. The dispatcher picks READY cards on each tick and
moves them to RUNNING according to the board's dispatch policy (see below).

### RUNNING

An agent has been assigned and is actively working. The card's `body` field is the
agent's working scratchpad — it updates this as it makes progress. You can watch this
field in the UI to see live progress without polling the agent directly.

Transition out (two paths):

- Agent completes → DONE.
- Agent cannot proceed without human input → BLOCKED (with a reason written to
  `blocked_reason`).

### BLOCKED

The agent has stopped and is waiting. The `blocked_reason` field explains what it needs.
Common reasons:

- Needs approval to run a destructive operation (HotL gate).
- External event has not arrived (webhook, approval email, CI result).
- Ambiguous requirements — agent surfaced the ambiguity rather than guessing.

Transition out: a human resolves the block (approves, provides context, or closes the
card as DONE/invalid). The dispatcher re-queues the card as READY.

### DONE

Terminal. The card is immutable after a five-minute grace period (allows last-second
corrections). DONE cards feed the outcome-telemetry pipeline; the full state-transition
log is the attribution chain for reporting.

## Dispatch policy

Each board has one of three dispatch strategies. Set it in the board configuration UI
or via `POST /v1/boards` with `dispatch_policy`:

### `fifo` (default)

First-in-first-out within READY. The oldest card (by insertion time) is dispatched
first. Predictable and fair; good default when cards have similar priority.

### `priority`

Cards carry a numeric priority field (0–255; higher = more urgent). The dispatcher
always picks the highest-priority READY card. Ties are broken by insertion order.
Use this when some alerts (SEV-1 PagerDuty) must jump the queue.

### `round-robin`

Distributes work evenly across available agents. Useful when agents specialise by
affinity tag and you want balanced load rather than pure priority ordering.

## Worker pool sizing

`pool_size` is the maximum number of RUNNING cards at any one time on a board. The
dispatcher will not dispatch more than `pool_size - len(RUNNING)` cards per tick.

**Starting guidance**:

| Scenario | Recommended pool_size |
|----------|----------------------|
| Single-node, light workload | 3–5 |
| Standard production | 5–10 |
| High-volume alert processing | 10–20 |
| Limit: half of PG connection pool | hard ceiling |

Do not set `pool_size` above half your Postgres `max_connections` budget reserved for
`xiaoguai-core`. Each RUNNING card may hold a Postgres connection while the agent
writes card body updates.

The dispatcher tick interval defaults to 2 seconds. Lower it (e.g., 500ms) for
latency-sensitive workflows; this increases PG query rate proportionally.

## Multi-board patterns

### Per-team boards

Create one board per team. devops gets a `platform-oncall` board; security gets a
`security-review` board. Teams see only their own cards (RBAC role scoped to board_id
is planned for v1.4.1).

### Per-project boards

Short-lived boards for a migration or launch. Archive the board when done; all card
history is retained in outcome-telemetry.

### Per-pack boards

Each pack that auto-creates cards targets a specific board. This keeps pack-generated
work separate from manually-created work and makes per-pack throughput metrics clean.

## Integration with packs

Packs create cards via the `task_create` MCP tool:

```
task_create(
  board_id = "incident-board-uuid",
  title    = "PagerDuty SEV-1: database latency spike",
  body     = "Alert ID: pd-xxx\nRunbook: ...",
  column   = "TRIAGE",
  priority = 200,
  affinity = "infra"
)
```

The `devops-oncall` pack fires this on every PagerDuty alert above a configurable
severity threshold. The `incident-triage` pack does the same for Grafana alerts.

Operators configure which board each pack targets and whether cards land in TRIAGE
(default, requires human promotion) or jump directly to TO-DO (set `auto_triage = true`
on the board and add the pack to the trusted-pack whitelist).

## Integration with HotL (Human-on-the-Loop)

When an agent reaches a decision point that requires human approval — deleting a
resource, sending an external communication, applying a change to production — it
transitions the card to BLOCKED and writes a structured `blocked_reason`:

```
Waiting for approval to: DROP TABLE legacy_sessions
Impact: irreversible. Approver role required: db-admin.
HotL request ID: hotl-abc123
```

The HotL subsystem surfaces this as an approval request in the admin UI and (if
configured) via IM. On approval, the card returns to READY; the dispatcher re-assigns
it to an agent (may be the same agent or a different one depending on affinity).

On rejection, the card moves to DONE with outcome `rejected`; the attribution chain
records who rejected and why.

## Integration with outcome telemetry

Every column transition records an outcome event:

```
card_events row:
  card_id       = <uuid>
  from_col      = RUNNING
  to_col        = DONE
  actor         = agent:xiaoguai-agent-07
  reason        = "Task completed successfully"
  ts            = 2026-05-25T14:32:01Z
```

This makes the full lifecycle of a card — who created it, which agent ran it, how long
it spent in each column, whether it was blocked and by whom — queryable via the
`/v1/outcomes` API and visible in the Grafana dashboard.

Use case: "Which agent class resolves SEV-1 incidents fastest?" Filter card_events by
pack=devops-oncall, group by assigned_to, aggregate RUNNING duration.

## Honest gaps (v1.4 ship state)

The following are **not** available at v1.4 ship:

- Card-to-card dependencies (blocking relationships between cards).
- Per-board RBAC (v1.4.1 planned).
- External sync adapters (push to Linear/Jira — deferred to v1.5 pack).
- Card expiry / automatic archival (manual archive via `xg tasks archive` only).
- Dispatcher fairness fallback for affinity exhaustion (open question — see ADR-0019).
- Workspace sub-tenant scoping (open question — see ADR-0019).

If you need any of these today, open a GitHub issue referencing ADR-0019.

## CLI quick reference

```bash
# List boards
xg tasks boards list

# List cards in a column
xg tasks cards list --board <id> --column READY

# Move a card manually
xg tasks cards move --card <id> --to TO-DO

# Trigger dispatcher immediately
xg tasks dispatch --board <id>

# Show card event history (attribution chain)
xg tasks cards events --card <id>
```

Full reference: `xg tasks --help`.

## Cross-references

- [ADR-0019](../../architecture/adr/0019-task-board.md) — design decision record for this feature.
- [ADR-0006](../../architecture/adr/0006-mcp-tasks-primitive.md) — MCP async task semantics the board builds on.
- [Day-2 Operations](day2.md) — monitoring RUNNING queue depth in Grafana.
- ROADMAP v1.4 — this feature is the primary v1.4 deliverable.
