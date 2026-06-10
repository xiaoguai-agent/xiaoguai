# Implementation plan — T6: Event-driven self-healing loop

| | |
|---|---|
| Date | 2026-06-10 |
| Status | **APPROVED (owner blanket "全部执行" 2026-06-10)** — open questions flagged in the PR |
| Parent | `docs/plans/2026-06-09-capability-upgrade.md` §2-F / §3-T6 (after T4+T5) |
| Hard constraints | DEC-033 unchanged; daemon-resident under `xiaoguai serve` |

## 0. Goal

Wire the existing parts into one governed loop: **alert → incident →
analysis (consult) → approved fix (execute, HotL-gated) → report** — the
AIOps Monitor→Analyst→Executor paradigm on xiaoguai's governance base.
**This is wiring, not new engines** (integrate-first §0.1): the explore pass
found ~90% of the parts already shipping.

## 1. What exists (verified, explore 2026-06-10)

- **Scheduler is the Monitor**: daemon-resident `JobRunner` with Interval/
  Cron/FileWatch/Webhook/GitPush/DbPoll/Proactive triggers; jobs already run
  agent turns via `RuntimeJobExecutor` with `scheduler:<job_id>` attribution
  and per-run sessions (`runtime_executor.rs:85-128`, wiring
  `xiaoguai-core/lib.rs:461-616`).
- **Incident normalization already shipped**: `xiaoguai-api/src/incidents.rs`
  — `Incident` (Sentry+Datadog sources), `RcaDraft`, `PrDraft`,
  `ImNotification` renderers (64-879). **No persistence, no route, no
  dispatch** — pure adapter awaiting wiring.
- **Analyst/Executor governance split is exactly T5**: Analyst runs in
  **consult mode** (read-only toolbox + ConsultGate); Executor runs in
  **execute mode** (per-tool HotL gates).
- **Executor safety net**: `xiaoguai-coding` `Workspace::checkpoint/rollback`
  + `GovernedTools` (gate + step audit) already shipped.
- **Watch/anomaly**: `xiaoguai-watch` fires `WatchEvent`s (channel, no sink);
  `xiaoguai-anomaly` registry exists unwired — both can feed the same
  incident path later; v1 takes webhooks + a manual POST.

## 2. Design — four thin glue pieces + 1 migration

### 2.1 Migration 0033 — incident state

```text
incidents(id TEXT PK, source TEXT, external_id TEXT, title TEXT,
  severity TEXT, project TEXT, environment TEXT, occurred_at TEXT,
  raw_payload TEXT, status TEXT CHECK(status IN
    ('open','analyzing','awaiting_approval','repairing','resolved','failed','dismissed'))
    DEFAULT 'open',
  created_at TEXT, updated_at TEXT)
incident_rcas(id TEXT PK, incident_id TEXT FK, session_id TEXT,
  summary TEXT, root_cause TEXT, confidence REAL, action_items TEXT/*json*/,
  raw_markdown TEXT, created_at TEXT)
incident_repairs(id TEXT PK, incident_id TEXT FK, rca_id TEXT FK,
  session_id TEXT, ok BOOL, summary TEXT, created_at TEXT)
```

Dedup: unique partial index on `(source, external_id)` for non-terminal
statuses — a re-fired alert updates `updated_at` instead of opening a twin.

### 2.2 GLUE-1 — ingest

- `POST /v1/incidents/ingest/{source}` (source = sentry|datadog|manual):
  normalize via the existing `IncidentSource` impls (manual = body already in
  `Incident` shape), persist, audit `incident.open`. Token-gated like the
  scheduler webhook routes (reuse `webhook_token_validator`).
- `GET /v1/incidents` + `GET /v1/incidents/{id}` (joined rca/repair summaries)
  for UI/CLI visibility. 503-when-absent pattern.

### 2.3 GLUE-2 — Analyst (consult)

- `IncidentPipeline` (in `xiaoguai-api`, LoopController-style): on ingest (or
  `POST /v1/incidents/{id}/analyze` re-run), spawn an **Analyst turn**: a
  dedicated session (`incident:<id>` attribution label), **TurnMode::Consult**,
  prompt = incident details + instruction to produce an RCA in the
  `RcaDraft` markdown contract; parse via the existing `RcaDraft` parser;
  persist `incident_rcas`; status open→analyzing→awaiting_approval; audit
  `incident.analyzed`. Analysis failure → status stays `open` with audit
  `incident.analysis_failed` (retryable via the analyze route).

### 2.4 GLUE-3 — Executor (execute, approval-gated)

- **The approval point is explicit**: `POST /v1/incidents/{id}/approve-repair`
  (owner action — UI/CLI/curl) moves awaiting_approval→repairing and spawns
  the **Executor turn**: execute mode, prompt = incident + RCA action items;
  toolbox = the normal session toolbox (coding tools already governed:
  checkpoint before mutations is part of the prompt contract + the coding
  gate); every write tool passes HotL per policy. Outcome →
  `incident_repairs` + status resolved|failed; audit `incident.repaired` /
  `incident.repair_failed`.
- **No auto-execute in v1** — the human approves the jump from analysis to
  mutation. (A HotL-policy-driven auto path can come later; flagged §5.)

### 2.5 GLUE-4 — report

- `GET /v1/incidents/{id}/report` renders the existing RCA markdown
  (incident + RCA + repair outcome) via the shipped renderers. IM push reuses
  the existing notification path **only if trivially reachable** from the
  pipeline's deps; otherwise the report endpoint is the v1 deliverable and IM
  push is flagged as follow-up (no new sink plumbing in T6).

## 3. Tasks

| # | Task | Size | Verification |
|---|---|---|---|
| T6.1 | migration 0033 + incident store (trait + sqlite + memory) + dedup | S | repo unit tests incl. dedup upsert + status transitions |
| T6.2 | ingest/list/get routes + audit + token gate | S | route tests: sentry/datadog/manual payloads, dedup, 503/401 |
| T6.3 | IncidentPipeline: Analyst consult turn → RcaDraft parse → persist; analyze route | M | integration: MockBackend RCA script → rca row + status + audit; parse-failure path |
| T6.4 | approve-repair route + Executor execute turn → repairs + status; report route | M | integration: approve → executor runs (mock) → repair row + status + audit; report renders |
| T6.5 | docs: user-guide self-healing + runbook (wire Sentry/Datadog webhook → xiaoguai) | S | docs only |

Sequencing: T6.1 → T6.2 → T6.3 → T6.4 → T6.5. UI pane deferred (REST+CLI
visibility first; admin pane can come with T7/T8 wave if time allows — flagged).

## 4. Boundaries

- No anomaly/watch auto-wiring in v1 (they can POST the manual ingest route).
- No auto-execute; no PagerDuty source; no new IM sink plumbing.
- No new crates; DEC-033 intact; everything under `xiaoguai serve`.

## 5. Open questions (defaults chosen, flagged in PR)

1. **Auto-repair policy**: v1 = human approval always. Later: a HotL policy
   scope (`incident.auto_repair`) could allow auto-execute for low-severity.
2. **Analyst tool surface**: consult subset of the session toolbox (it can
   read files/logs/RAG but mutate nothing). Good default; revisit if RCA
   quality needs more probes.
3. **Admin-ui incidents pane**: deferred from T6 (REST first).
