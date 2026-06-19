# Incidents admin pane (T6 deferred UI) — 2026-06-19

## Context & goal

T6 self-healing (#277, migration 0033) shipped the incident **backend**
(ingest / list / get / analyze / approve-repair / report) but explicitly
deferred the admin UI — `crates/xiaoguai-api/src/incident_store/mod.rs` and the
T6 plan note "REST+CLI visibility first; admin pane can come later". Today the
only way to see an incident, review its RCA, or approve a repair is `curl`.

This plan adds the owner-facing **Incidents** admin pane so the human-in-the-loop
approval gate (`approve-repair`) is usable from the UI.

Owner decision 2026-06-19: build the **full pane** (list + detail + manual
create + analyze + approve-repair + report), not just a manual-ingest form — a
create form with no way to see/act on the result is half a feature.

## Constraints

- **DEC-033**: single binary, embedded SQLite, single owner, `:7600`. No new
  deps, no new persistence.
- Backend is DONE; this is additive **frontend + one small owner-authed route**.
- Follow `~/.claude/rules/*`: immutable data, small files (<400 LOC), explicit
  errors, validate at boundaries. Match the existing admin-pane idiom
  (`panes/ExpertTeams.tsx`, `panes/Personas.tsx`).

## Backend — two new routes

Current incident routes (verified `routes/mod.rs:243-325`, `routes/incidents.rs`):

| Method | Path | Auth group | Response |
|---|---|---|---|
| POST | `/v1/incidents/ingest/{source}` | `X-Xiaoguai-Token` (`public_v1`) | `{incident, was_duplicate}` |
| GET | `/v1/incidents?status=` | owner (`v1`) | `IncidentRecord[]` |
| GET | `/v1/incidents/{id}` | owner | `IncidentDetails {incident, rcas[], repairs[]}` |
| POST | `/v1/incidents/{id}/analyze` | owner | `{rca: RcaRecord, status}` |
| POST | `/v1/incidents/{id}/approve-repair` | owner; body `{rca_id}` | `{repair: RepairRecord, status}` |
| GET | `/v1/incidents/{id}/report` | owner | `text/markdown` |

**Gap:** manual ingest is token-gated and sits OUTSIDE owner-auth (observability
platforms can't do HTTP Basic). Making the owner mint + hold a webhook token in
the admin UI just to file an incident is awkward and a token-exposure smell.

**Add `POST /v1/incidents` (owner-authed, in the `v1` group):** reuses the
existing `normalize_manual(body)` + `store.ingest`, stamps `source="manual"`,
returns `{incident, was_duplicate}` 201 / 400-on-malformed. ~30 LOC mirroring
`ingest_incident`'s manual branch **minus** the token gate. The token-gated
`ingest/{source}` path stays untouched for external platforms.

- Audit: stamp `incident.open` exactly like `ingest_incident` (`incidents.rs:299`).

**Also add `POST /v1/incidents/{id}/dismiss` (owner-authed):** reuses
`store.set_status(id, Dismissed)` — the status machine already allows any
non-terminal → `dismissed` (`incident_store/mod.rs:101`). Returns the updated
`IncidentRecord`; the store maps an already-terminal incident to
`InvalidTransition` → 409. Audit `incident.dismissed`.

- **TDD** (`crates/xiaoguai-api/tests/incidents.rs`): owner create with
  title-only body → 201 + `open`; no title → 400; create → list shows it.
  Dismiss an `open` incident → `dismissed`; dismiss a `resolved` one → 409.

## Shared types + client (`frontend/shared/src/index.ts`)

Add TS types mirroring the Rust structs (snake_case wire form, from
`incident_store/mod.rs` + `incidents.rs`):

- `IncidentStatus = 'open'|'analyzing'|'awaiting_approval'|'repairing'|'resolved'|'failed'|'dismissed'`
- `Severity = 'critical'|'high'|'medium'|'low'`
- `IncidentRecord { id, source, external_id, title, severity, project, environment: string|null, occurred_at, raw_payload: unknown, status: IncidentStatus, created_at, updated_at }`
- `RcaRecord { id, incident_id, session_id, summary, root_cause, confidence: number, action_items: unknown, raw_markdown, created_at }`
- `RepairRecord { id, incident_id, rca_id, session_id, ok: boolean, summary, created_at }`
- `IncidentDetails { incident: IncidentRecord, rcas: RcaRecord[], repairs: RepairRecord[] }`
- `CreateIncidentRequest { title: string; severity?: Severity; project?: string; environment?: string; url?: string; occurred_at?: string; raw?: unknown }`
- analyze resp `{ rca: RcaRecord; status: IncidentStatus }`, approve resp `{ repair: RepairRecord; status: IncidentStatus }`

Client methods (`request<T>` / `requestNoContent` idiom, lines ~2279-2301):

- `listIncidents(status?)` → GET `/v1/incidents`
- `getIncident(id)` → GET `/v1/incidents/{id}`
- `createIncident(req)` → POST `/v1/incidents`
- `analyzeIncident(id)` → POST `/v1/incidents/{id}/analyze`
- `approveRepair(id, rca_id)` → POST `/v1/incidents/{id}/approve-repair`
- `dismissIncident(id)` → POST `/v1/incidents/{id}/dismiss`
- `incidentReport(id)` → GET `/v1/incidents/{id}/report` — **text**, not JSON →
  check for a `requestText` helper; add one if absent (small).

- **TDD**: shared vitest (mirror `orchestrate.test.ts`) — each method hits the
  right path/method/body and parses the typed response.

## Admin pane (`frontend/admin-ui/src/panes/Incidents.tsx`)

Mirror the ExpertTeams idiom exactly:

- `LoadState` discriminated union `loading | ok | unavailable(503) | error`;
  `client ?? defaultClient` injection via `Pick<XiaoguaiClient, ...>` prop;
  `useCallback` refresh + `useEffect`; global `styles.css` classes
  (`.pane/.toolbar/.drawer*/.alert/.muted/.kind-tag`); `RequireScope` + `PaneIntro`.
- **List**: table (title, source, severity chip, status chip, created_at) +
  status filter dropdown + refresh.
- **"+ New incident" drawer**: title (required) + optional
  severity/project/environment → `createIncident` → refresh.
- **Detail drawer** (row click): incident fields + `raw_payload` (collapsed);
  RCA list (summary / root_cause / confidence / action_items); repair history
  (ok badge + summary).
  - status `open` → **Analyze** button → `analyzeIncident` (spinner — the turn
    runs server-side, takes seconds) → refresh.
  - status `awaiting_approval` + has RCA → **Approve repair** → confirm modal →
    `approveRepair(id, latestRca.id)` → refresh.
  - any non-terminal status → **Dismiss** → confirm modal → `dismissIncident(id)`
    → refresh (soft "close without acting").
  - **View report** → `incidentReport` → render markdown (reuse an existing
    renderer; else `<pre>`. No new dep).
- Nav + route in `App.tsx` (`/incidents`). i18n `pane.incidents.*` + `nav.incidents`
  across **en / zh-CN / ja** (parity is gated by the merge check).
- **Scopes**: define incident scopes consistent with personas/teams — verify the
  scope catalog source first; likely `incidents.write` (create/analyze) +
  `incidents.approve` (approve-repair). Resolve when wiring `RequireScope`.
- **TDD**: admin-ui vitest if a pane component-test harness exists (check
  `Personas`/`ExpertTeams` `*.test.tsx`).

## e2e (`frontend/e2e/tests/admin-ui/admin-incidents.spec.ts`)

Mirror `admin-personas.spec.ts`: mutable mock store via
`page.route('**/v1/incidents**')`, mock `/v1/admin/me/scopes` to reveal write
buttons. Cases: list renders; new-incident drawer creates → row appears; open
detail → Analyze (mock 200 `{rca, status: awaiting_approval}`) → Approve repair
(mock 200 `{repair, status: resolved}`) → status updates. Hermetic;
non-blocking gate.

## Phases & verification points

| # | Phase | Verify |
|---|---|---|
| 0 | Backend `POST /v1/incidents` + `…/{id}/dismiss` + tests | `cargo test -p xiaoguai-api incidents` green |
| 1 | Shared types + client + vitest | `pnpm --filter @xiaoguai/shared test`, `tsc` green |
| 2 | Admin pane + nav + i18n×3 | `pnpm --filter @xiaoguai/admin-ui test`, `tsc`, 3-locale parity green |
| 3 | e2e spec | runs against the mocked stack |
| 4 | Gate + docs | `cargo fmt --check` + `clippy --workspace -D warnings` (doc_markdown backticks!), frontend lint/tsc; update `docs/user-guide/self-healing.md` ("Incidents pane"); open PR |

## Risks / out of scope

- `analyze`/`approve-repair` run agent turns **synchronously in-request** → pane
  needs pending UX; real runs need a wired LLM (e2e mocks it).
- No auto-repair; no hard-delete. Dismiss IS included (the soft "close without
  acting" path); there is no row-deletion — terminal incidents stay for the audit
  trail.
- Don't touch the token-gated `ingest` path or migration 0033.
