# Incident Self-Healing — Admin Pane Flow

The T6 self-healing backend (incident ingest → Analyst RCA → human-approved
Executor repair; capability wave #277, migration 0033) shipped REST + CLI only.
The **admin-ui Incidents pane** (PR for `feat/incidents-pane`, plan
[`docs/plans/2026-06-19-incidents-pane.md`](../../plans/2026-06-19-incidents-pane.md))
makes the human-in-the-loop approval gate operable from the browser.

Two trust boundaries meet here and must stay separate:

- **owner-authed `/v1`** — everything the pane drives (list/get, **create**,
  analyze, approve-repair, **dismiss**, report). The owner is already
  authenticated (DEC-033, single owner), so no extra token.
- **token-gated `public_v1`** — `POST /v1/incidents/ingest/{source}` for
  external observability platforms (Sentry/Datadog) that can't do HTTP Basic;
  guarded by `X-Xiaoguai-Token`. The pane never uses this path.

Manual create from the pane uses the **new owner-authed `POST /v1/incidents`**
(reuses `normalize_manual`), NOT the webhook — so a browser never holds a
long-lived webhook token, and both paths still produce identical rows via the
one `IncidentStore::ingest`.

## 1. Architecture — components & trust boundaries

```mermaid
flowchart TB
    Op([Operator])
    subgraph FE["admin-ui — Incidents pane"]
      direction LR
      L["List<br/>status filter"]
      D["Detail drawer<br/>RCA + repairs"]
      N["New incident<br/>form"]
    end
    C["@xiaoguai/shared client<br/>list · get · create · analyze · approve · dismiss · report"]
    subgraph API["xiaoguai-api"]
      direction LR
      OW["owner-authed /v1<br/>create · analyze · approve-repair · dismiss · list/get · report"]
      IN["public_v1 token-gated<br/>POST /v1/incidents/ingest/:source"]
    end
    EXT([Sentry / Datadog])
    PIPE["IncidentPipeline<br/>Analyst consult · Executor execute"]
    STORE[("IncidentStore → SQLite<br/>migration 0033")]
    AUD["HMAC audit chain"]

    Op --> FE --> C --> OW
    EXT -- "X-Xiaoguai-Token" --> IN
    OW -- "analyze / approve" --> PIPE --> STORE
    OW -- "create / dismiss / list / get" --> STORE
    IN --> STORE
    OW -. "incident.open / dismissed / repaired" .-> AUD
    IN -. "incident.open" .-> AUD
```

## 2. Incident lifecycle — status machine + driving action

Each transition is gated by `IncidentStatus::can_transition_to`
(`crates/xiaoguai-api/src/incident_store/mod.rs`). Live states hold the
`(source, external_id)` dedup slot; `resolved` / `failed` / `dismissed` are
terminal and immutable. The label on each edge is the pane action / endpoint
that drives it.

```mermaid
stateDiagram-v2
    direction LR
    [*] --> open : create or ingest
    open --> analyzing : Analyze
    analyzing --> awaiting_approval : RCA ok
    analyzing --> open : RCA fail, retryable
    awaiting_approval --> repairing : Approve repair
    repairing --> resolved : repair ok
    repairing --> failed : repair fail
    open --> dismissed : Dismiss
    analyzing --> dismissed : Dismiss
    awaiting_approval --> dismissed : Dismiss
    repairing --> dismissed : Dismiss
    resolved --> [*]
    failed --> [*]
    dismissed --> [*]
```

- **Analyze** → `POST /v1/incidents/{id}/analyze` (Analyst consult turn; 409
  unless `open`; 502 on agent/RCA-contract failure → reverts to `open`).
- **Approve repair** → `POST /v1/incidents/{id}/approve-repair` with
  `{rca_id}` (the RCA the owner reviewed, #284); 409 on a stale `rca_id`.
- **Dismiss** → `POST /v1/incidents/{id}/dismiss` (any non-terminal →
  `dismissed`; 409 if already terminal).

## 3. Sequence — create → analyze → approve

```mermaid
sequenceDiagram
    autonumber
    participant Op as Operator
    participant Pane as Incidents pane
    participant API as xiaoguai-api<br/>(owner-authed)
    participant Pipe as IncidentPipeline
    participant Store as IncidentStore<br/>(SQLite)
    participant Audit as HMAC audit chain

    Op->>Pane: New incident (title)
    Pane->>API: POST /v1/incidents
    API->>Store: normalize_manual + ingest → open
    API->>Audit: incident.open
    API-->>Pane: 201 {incident, was_duplicate}

    Op->>Pane: Analyze
    Pane->>API: POST /v1/incidents/{id}/analyze
    API->>Pipe: Analyst consult turn (LLM)
    Pipe->>Store: insert RCA, status = awaiting_approval
    API-->>Pane: 200 {rca, status}

    Op->>Pane: Approve repair (rca_id)
    Pane->>API: POST /v1/incidents/{id}/approve-repair
    API->>Pipe: Executor execute turn
    Pipe->>Store: insert repair, status = resolved | failed
    API->>Audit: incident.repaired
    API-->>Pane: 200 {repair, status}

    Note over Op,Store: Dismiss is the soft exit — any non-terminal state →<br/>POST /v1/incidents/{id}/dismiss → dismissed (no agent turn)
```

## Related

- **HLD decision**: `xiaoguai-agent-design/docs/hld.md` → DEC-040 (incident admin pane).
- **Plan**: [`docs/plans/2026-06-19-incidents-pane.md`](../../plans/2026-06-19-incidents-pane.md)
- **Source**:
  - Routes: `crates/xiaoguai-api/src/routes/incidents.rs` (+ mount in `routes/mod.rs`)
  - Store + status machine: `crates/xiaoguai-api/src/incident_store/`
  - Pipeline: `crates/xiaoguai-api/src/incident_pipeline.rs`
  - Normalizer: `crates/xiaoguai-api/src/incidents.rs`
  - Pane: `frontend/admin-ui/src/panes/Incidents.tsx`; client `frontend/shared/src/index.ts`
- **User guide**: [`docs/user-guide/self-healing.md`](../../user-guide/self-healing.md)
- **Migration**: `crates/xiaoguai-storage/migrations/0033_incidents.sql`
