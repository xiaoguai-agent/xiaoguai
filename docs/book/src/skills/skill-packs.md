# Skill Packs

A **skill pack** is a self-describing bundle that wires agent configs,
inbound webhook sources, output adapters, and Tera templates into a named,
versioned unit that an operator can install per-tenant from the admin UI or
the API.

> **Status — v1.2.28 (current):** Install operations record a row in the
> `installed_skill_packs` table and the row persists across restarts.
> **Agent activation, inbound webhook registration, and template wiring are
> NOT yet live.** The runtime pack loader that reads those rows and activates
> the pack at runtime is tracked for v1.3. Until then, installing a pack
> declares intent; the agents and webhooks it declares do not yet start
> automatically.

---

## Anatomy of a pack

Every pack lives under `packs/<name>/` and has at minimum one file:
`pack.yaml`.

### Directory layout

```
packs/<name>/
├── pack.yaml          ← manifest (required)
├── agents/            ← one YAML per agent the pack declares
│   └── <name>.yaml
├── inbound/           ← webhook source configs
│   └── <source>.yaml
├── outputs/           ← output adapter configs (MCP tool call or HTTP POST)
│   └── <adapter>.yaml
├── templates/         ← Tera templates for rendered output bodies
│   └── <template>.md.j2
├── watches/           ← declarative watch specs (for anomaly packs)
├── anomalies/         ← rolling-window anomaly specs
├── migrations/        ← pack-owned SQL migrations (applied on install)
├── corpus/            ← RAG corpus documents (rag-* packs only)
├── prompts/           ← RAG prompt templates (rag-* packs only)
└── tests/             ← pack-level integration tests
```

RAG knowledge packs (`rag-legal`, `rag-finance`, `rag-hr`) omit
`agents/`, `inbound/`, and `outputs/` — they only add a corpus to the
RAG store and a set of retrieval prompts.

### pack.yaml schema

A pack manifest has a fixed set of top-level keys. Unknown keys are ignored
for forward compatibility.

| Key | Type | Required | Description |
|-----|------|:--------:|-------------|
| `name` | string | yes | Unique slug matching the directory name |
| `version` | semver string | yes | Pack version, independent of xiaoguai version |
| `description` | string | yes | One-paragraph description shown in the marketplace UI |
| `kind` | `scaffold-only` or omitted | no | `scaffold-only` marks packs that need a real corpus before production use |
| `requires` | object | no | Platform version + feature flags + env vars the pack needs |
| `depends` | object | no | Named runtime crate dependencies (orchestrator, scheduler, IM adapters) |
| `agents` | list of `{ref:}` or `{path:}` | no | Agent definitions to register |
| `inbound` / `sources` | list | no | Webhook source configs to register |
| `outputs` | list | no | Output adapter configs |
| `watches` | list | no | WatchSpec declarations |
| `anomalies` | list | no | AnomalySpec declarations |
| `migrations` | list | no | SQL migration paths applied in order on install |
| `plan` | list of steps | no | Execution plan linking agents and outputs |
| `config` | object | no | Default values for operator-tuneable knobs |
| `feature_flag` | string | no | Env var that must be `true` to activate routes |
| `dashboards` | list | no | Admin-UI dashboard widget specs |

### Agent YAML (`agents/<name>.yaml`)

```yaml
name: reviewer
kind: llm-agent            # llm-agent | mcp-tool | supervisor | worker
mcp_servers: [github_pr]   # MCP servers the agent's toolbox exposes
system_prompt: |
  You are a thorough code reviewer. …
user_prompt_template: |
  PR #{{ pr_number }} — {{ pr_title }}
  …
tools:
  - get_pr_diff
budget:
  max_llm_calls: 3
  max_output_tokens: 4096
```

### Inbound webhook YAML (`inbound/<source>.yaml`)

```yaml
name: github-pr-webhook
kind: webhook
route_id: "github-pr-${PACK_INSTANCE_ID}"  # resolved at deploy time
event_filter:
  header: "X-GitHub-Event"
  values: ["pull_request"]
signature:
  algorithm: hmac-sha256
  header: "X-Hub-Signature-256"
  secret_env: GITHUB_WEBHOOK_SECRET
extract:
  pr_number: "$.number"
  head_sha:  "$.pull_request.head.sha"
  …
```

The webhook config maps directly to a `WebhookSourceAdapter` row in
`xiaoguai-scheduler`. Routes are registered per-tenant under
`POST /v1/scheduler/webhooks/<route_id>`.

### Output adapter YAML (`outputs/<adapter>.yaml`)

```yaml
name: post-review
kind: mcp-tool             # mcp-tool | http-post | im-message
mcp_server: github_pr
tool: post_pr_review
summary_template: templates/review-summary.md.j2
```

### Execution plan

The `plan:` block in `pack.yaml` defines a directed acyclic graph of steps.
Each step names an agent or output adapter and lists its `deps`:

```yaml
plan:
  - id: review
    agent: reviewer
  - id: challenge
    agent: challenger
    deps: [review]
  - id: post
    output: post-review
    deps: [review, challenge]
```

Steps without `deps` run immediately; steps with `deps` wait for all
dependencies to complete.

---

## The 7 shipped packs

### `pr-review`

A webhook-triggered, two-agent pipeline for GitHub pull requests. On every
`pull_request` event (`opened` or `synchronize`) the **reviewer** agent
fetches the diff via the `github_pr` MCP server and emits a structured list
of inline findings. The **challenger** agent critiques the reviewer's output
for gaps and unstated assumptions. The **post-review** output adapter merges
both lists, maps severity to GitHub review event (`CHANGES_REQUESTED`,
`COMMENT`, or `APPROVE`), and calls `post_pr_review` to publish the combined
review. Requires `GITHUB_TOKEN` and `GITHUB_WEBHOOK_SECRET`.

### `incident-triage`

Ingests Sentry issues and Datadog alert webhooks (per-tenant HMAC-SHA256
token gating). A **triage-agent** correlates the alert with recent git
commits and audit-log entries to draft a root-cause analysis, then emits a
draft GitHub PR with the RCA and an IM notification to the configured
channel (Feishu, DingTalk, or WeCom). Requires xiaoguai >= 0.12.1 for
per-tenant webhook token middleware and at least one IM adapter. PagerDuty
ingestion is deferred (see `docs/plans/incident-triage-backlog.md`).

### `ar-collections`

Proactive accounts-receivable monitoring. A declarative `WatchSpec`
(`watches/dso-over-60.yaml`) polls the AR aging table; an `AnomalySpec`
detects DSO drift above configurable thresholds. When drift is detected the
**dunning-drafter** agent generates graded dunning emails keyed to overdue
bucket (30/60/90 days). All email actions are HOTL-gated — zero sends
without human approval from the HOTL queue. Requires xiaoguai >= 1.3.1 and
the `watch`, `anomaly`, and `outcome-telemetry` feature flags. Ships one
SQL migration (`migrations/0001_ar_aging.sql`) applied on install.

### `hr-onboarding`

Multi-agent onboarding automation using the orchestrator Supervisor pattern.
On an employee's start date the scheduler fires a cron trigger; a
**coordinator** agent decomposes "onboard \<name\>" into four subtasks
(account provisioning, meeting scheduling, welcome messaging, buddy
assignment) and fans them out to four specialist worker agents. Feishu
`post_message` and `create_group_chat` are real; Okta, Google Workspace,
and Google Calendar integrations are mock hooks with documented substitution
points. Supports Feishu, DingTalk, and WeCom IM adapters via config.

### `rag-legal`

A scaffold RAG knowledge pack for legal teams. Provides a public-domain
corpus (CommonAccord templates, FOSS license texts, CFAA excerpt) chunked
with the `text-default` preset (512-token chunks, 64-token overlap). Uses
`nomic-embed-text` for embeddings and `bge-reranker-v2-m3` for reranking
(top-20 recall, top-5 after rerank). Ships retrieval prompts tuned for
contract Q&A. Replace the `corpus/` directory with your tenant's actual
legal documents before production use — the shipped corpus is illustrative
only (`kind: scaffold-only`).

### `rag-finance`

A scaffold RAG knowledge pack for finance teams. Corpus covers SEC EDGAR
public filings (10-K/10-Q excerpts), IFRS summaries, and US GAAP ASC
reference material. Uses the `pdf-heavy` chunking preset suited to long
annual-report prose. Same embedding and reranking stack as `rag-legal`.
Replace the corpus with tenant-specific filings before production use.

### `rag-hr`

A scaffold RAG knowledge pack for HR teams. Corpus covers US DOL FMLA/FLSA
fact sheets, EEO/Title VII summaries, and SHRM-style policy templates (all
original MIT work). Uses the `text-default` preset. Prompts are tuned for
policy Q&A and employee self-service questions. Replace the corpus before
production use.

---

## Install flow

### API

**Browse the catalog** (no auth required):

```http
GET /v1/skills/catalog
```

Returns the full catalog baked into the binary, including `requires`,
`knobs` (typed operator-tuneable fields), and `category` for UI grouping.

**Install a pack for a tenant:**

```http
POST /v1/skills/install
Content-Type: application/json

{
  "tenant_id": "<uuid>",
  "pack_slug": "rag-hr",
  "config": { "top_k": 10 }
}
```

Returns the persisted row on success, `409 Conflict` if the pack is already
installed for that tenant, `404 Not Found` for an unknown slug.

**List installed packs:**

```http
GET /v1/skills/installed?tenant=<tenant_uuid>
```

**Uninstall:**

```http
DELETE /v1/skills/install/<row-uuid>
```

Returns `{ "deleted": "<id>" }` on success, `404` if the row does not exist.

### What gets recorded

A successful install writes one row to the `installed_skill_packs` table
(migration `0015`):

```sql
CREATE TABLE installed_skill_packs (
    id           UUID        PRIMARY KEY,
    tenant_id    UUID        NOT NULL,
    pack_slug    TEXT        NOT NULL,
    version      TEXT        NOT NULL,
    config       JSONB       NOT NULL DEFAULT '{}'::jsonb,
    installed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, pack_slug)
);
```

The `config` column stores the `knob` overrides the operator supplied at
install time as free-form JSONB. Unknown keys are accepted for forward
compatibility with newer pack versions.

### What is NOT yet activated (v1.2 caveat)

Installing a pack currently:

- records the `installed_skill_packs` row
- validates the slug against the baked-in catalog
- persists operator knob overrides

Installing a pack does **not** yet:

- register the pack's inbound webhook routes in the scheduler
- start or register the pack's agent definitions in the agent registry
- apply pack-owned SQL migrations (`migrations/` directory)
- activate the execution plan
- show pack dashboards in the admin UI

The runtime pack loader that reads `installed_skill_packs` and activates
each pack's components is planned for v1.3. Until then, treat the install
row as a declaration of intent. Individual packs (e.g., `pr-review`) can
be activated manually by following their README and registering their
components via the existing MCP, scheduler, and agent APIs.

### Admin UI marketplace

The **Skills** pane in the admin UI renders the catalog as a browseable
card grid, grouped by `category` (`dev`, `ops`, `finance`, `hr`, `rag`).
Each card shows the pack description, version, `requires` checklist, and
any knob fields as a typed form. The **Install** button posts to
`/v1/skills/install`. The **Installed** tab lists the rows returned by
`/v1/skills/installed` for the current tenant, with an **Uninstall** button
per row.

---

## Authoring a new pack

### Minimal pack.yaml

```yaml
name: my-pack
version: "0.1.0"
description: >
  One paragraph explaining what the pack does and when to use it.

requires:
  env:
    - MY_SERVICE_TOKEN   # env vars the pack's runtime needs
  feature_flags:
    - scheduler          # xiaoguai feature flags that must be enabled

agents:
  - path: agents/my-agent.yaml

inbound:
  - ref: inbound/my-webhook.yaml

outputs:
  - ref: outputs/my-output.yaml

plan:
  - id: run
    agent: my-agent
  - id: deliver
    output: my-output
    deps: [run]
```

### Directory checklist

1. Create `packs/<name>/pack.yaml` with at minimum `name`, `version`,
   `description`.
2. Add agent configs under `agents/`. Each agent needs `name`, `kind`,
   and either `system_prompt` or a reference to a prompt template.
3. If the pack ingests webhooks, add an inbound config under `inbound/`
   with `kind: webhook`, `event_filter`, and `signature` settings.
4. Add any pack-owned SQL migrations under `migrations/` and reference
   them in `pack.yaml`'s `migrations:` list. Migrations run in the listed
   order when the runtime loader activates the pack (v1.3).
5. Add Tera templates under `templates/` and reference them from output
   adapters via `summary_template`.
6. Add the pack to `catalog/skill_packs.json` with `slug`, `name`,
   `description`, `version`, `category`, and any `requires` / `knobs`
   definitions, so it appears in the marketplace UI.

### Declaring an inbound webhook

A webhook inbound config maps a logical `route_id` to a scheduler webhook
row. The runtime loader will register the route at install time (v1.3).
Until then, register the route manually via `POST /v1/scheduler/webhooks`.

The `extract` block maps JSONPath expressions to named context variables
that the execution plan injects into agent prompt templates.

### Pack knobs

Knobs are operator-tuneable parameters defined in `catalog/skill_packs.json`
under the `knobs` key. Each knob has a `type` (`integer`, `boolean`, or
`string`), a `default`, and a `description`. The admin UI renders them as a
typed form at install time. Knob values are persisted in `config` JSONB and
passed to the pack's agents at activation time (v1.3).

```json
"knobs": {
  "top_k": { "type": "integer", "default": 20,
              "description": "Documents retrieved before reranking" },
  "dry_run": { "type": "boolean", "default": false,
               "description": "Log actions without sending" }
}
```

---

## Lifecycle

### List installed packs

```bash
curl -s "$XIAOGUAI_URL/v1/skills/installed?tenant=$TENANT_ID" \
  | jq '.[].pack_slug'
```

### Uninstall

```bash
curl -s -X DELETE "$XIAOGUAI_URL/v1/skills/install/$ROW_ID"
```

Uninstall removes the `installed_skill_packs` row. When the runtime loader
is active (v1.3) it will also deregister the pack's webhook routes and
agents for that tenant.

### Config diff

To see what knob overrides an installed pack is using:

```bash
curl -s "$XIAOGUAI_URL/v1/skills/installed?tenant=$TENANT_ID" \
  | jq '.[] | select(.pack_slug == "rag-hr") | .config'
```

To update knob overrides, uninstall and reinstall with the new `config`
body (idempotent in terms of data, but requires a brief gap between the
two calls to avoid the 409 unique constraint).

### Future: hot-reload model (v1.3)

The v1.3 pack loader will watch the `installed_skill_packs` table for
changes (via Postgres LISTEN/NOTIFY or a polling interval) and:

1. Apply any unapplied pack migrations.
2. Register the pack's inbound webhook routes in the scheduler.
3. Register the pack's agent configs in the per-tenant agent registry.
4. Activate the pack's execution plan.
5. Expose pack dashboards in the admin UI.

Uninstalls will trigger the reverse: routes and agents are deregistered,
the dashboard is removed. No xiaoguai restart will be needed.

---

## Related reading

- [Skills Catalog](overview.md) — registering individual MCP servers as tenant skills
- [REST API](../api/rest.md) — full `/v1/skills/*` endpoint reference
- [Architecture](../architecture.md) — how `McpSupervisor` and `xiaoguai-scheduler` relate to packs
- [Roadmap](../roadmap.md) — v1.3 pack-loader milestone
