# Self-healing incidents — alert → analysis → approved fix → report

T6 of the capability-upgrade plan (`docs/plans/2026-06-10-self-healing.md`).
xiaoguai turns an inbound alert into a governed incident workflow: a
**consult-locked Analyst** produces a root-cause analysis, a **human approves**
the jump to mutation, an **Executor under normal HotL policy** applies the fix,
and everything lands in the HMAC audit chain.

```
alert (Sentry / Datadog / anything) ──► incident (open)
   POST /v1/incidents/ingest/{source}        │
                                             ▼  POST /v1/incidents/{id}/analyze
                              Analyst turn — CONSULT mode (read-only)
                                             │  RCA persisted
                                             ▼  status: awaiting_approval
                              ★ HUMAN APPROVAL ★
                                             │  POST /v1/incidents/{id}/approve-repair
                                             ▼
                              Executor turn — EXECUTE mode (HotL-gated tools)
                                             │  repair persisted
                                             ▼  status: resolved | failed
                              GET /v1/incidents/{id}/report  → RCA markdown
```

## 1. Ingest

`POST /v1/incidents/ingest/{source}` with `source` = `sentry` | `datadog` |
`manual`. The route lives on the public surface and is token-gated exactly
like scheduler webhooks: send `X-Xiaoguai-Token` (mint via the scheduler token
admin API, route id `incidents`). Sentry/Datadog payloads are normalized by
the built-in adapters; `manual` accepts the normalized incident JSON directly
(`id`, `title`, `severity`, `source`, `occurred_at`, …).

The admin-ui **Incidents pane** files owner-initiated incidents through an
**owner-authed `POST /v1/incidents`** (same `manual` contract — only `title`
is required) rather than the token webhook: it already sits behind owner auth,
so no `X-Xiaoguai-Token` is minted into the browser (DEC-040).

Duplicate alerts (same `source` + `external_id`, incident not yet terminal)
are deduplicated: the route answers 200 with `was_duplicate: true` instead of
opening a twin.

## 2. Analysis (consult-locked)

`POST /v1/incidents/{id}/analyze` runs the Analyst as a single agent turn in
**consult mode** (T5): read-only toolbox subset + ConsultGate — the Analyst
can read files, search, query RAG, but every mutating tool is denied at the
gate. The reply must contain the RCA JSON contract (`summary`, `impact`,
`root_cause`, `timeline`, `action_items`, `confidence`, `evidence_refs`); it
is parsed, persisted, and the incident parks at `awaiting_approval`. A failed
or unparseable analysis reverts the incident to `open` (audited, retryable).

Token usage is attributed as `incident:<id>`.

## 3. Approval & repair (execute, HotL-gated)

Nothing mutates without a human: `POST /v1/incidents/{id}/approve-repair` is
the explicit approval. The Executor turn then runs with the normal toolbox
under your standing HotL policies — every write tool still passes
`tool_call.{name}` gates, so a strict policy can require per-action approval
even inside an approved repair. The prompt instructs the agent to checkpoint
the coding workspace before mutations (rollback stays available). The outcome
is recorded (`resolved` or `failed` — a failed attempt is a recorded fact,
not an HTTP error).

## 4. Report & visibility

- **Admin-ui Incidents pane** (DEC-040) — the owner review board: a
  status-filtered list, a detail drawer (incident + RCAs + repairs), and the
  operator actions (Analyze, Approve repair, Dismiss, View report) plus a
  manual "New incident" form. Diagram:
  `docs/architecture/diagrams/incident-pane-flow.md`.
- `GET /v1/incidents` (filter by `?status=`), `GET /v1/incidents/{id}`
  (incident + RCAs + repairs).
- `GET /v1/incidents/{id}/report` → `text/markdown` RCA report (summary,
  impact, root cause, timeline, action items, repairs).
- Audit chain actions: `incident.open`, `incident.analyzed`,
  `incident.analysis_failed`, `incident.repaired`, `incident.repair_failed`,
  `incident.dismissed`.

## 5. Status machine

`open → analyzing → awaiting_approval → repairing → resolved | failed`;
analysis failure returns to `open`; any non-terminal incident can be
`dismissed` (`POST /v1/incidents/{id}/dismiss` — the pane's Dismiss action).
Terminal incidents free the dedup slot (a recurring alert opens a fresh
incident).

## 6. Boundaries (v1)

- No auto-repair: the approval step is always human (a HotL-policy-driven
  auto path for low-severity incidents is a possible follow-up).
- Watch/anomaly subsystems aren't auto-wired yet — anything that can POST
  JSON can feed `ingest/manual`.
- The admin-ui **Incidents pane** (DEC-040) is the owner surface; REST +
  `curl`/CLI remain for automation and external (Sentry/Datadog) ingest.
