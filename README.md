# Xiaoguai 小怪

> **Rust-implemented, audit-first, scheduler-native local agent platform.**
>
> *Your Little Agent for Big Work · 小怪不小，能办大事*

**Documentation:** <https://xiaoguai-agent.github.io/xiaoguai/>

Xiaoguai is a self-hostable AI agent platform for technical individuals,
small teams, and anyone with a compliance or traceability constraint.
Every tool call writes an HMAC-chained audit row. Every scheduled job
carries a retry policy, a replayable transcript, and a reason field.
Every model interaction has a regression-eval safety net. The whole
thing ships as a single Rust binary plus Postgres and Valkey — no
Python runtime, no JVM, no JS server on the hot path.

We are not trying to out-prompt the prompt-magic vendors, out-polish
the UI vendors, or out-host the marketplace vendors. We compete on
engineering seriousness — *models are unreliable, but systems can be
reliable.*

## What makes it different

| Capability | Xiaoguai | n8n | Dify | OpenWebUI / LobeChat |
|---|---|---|---|---|
| **Audit-first console.** `Today` is the default landing page — every chat / IM / scheduled run with HMAC-chained audit metadata. Chat is a secondary entry. | First-class | Workflow runs only | Workflow runs only | Chat-first; audit not surfaced |
| **Scheduler-native (passive → reactive → proactive).** Cron + file watcher + webhook + LLM-initiated runs with per-user budget and a required `reason` field. | First-class | Strong on triggers / weak on agents | Cron only | None |
| **MCP two-way.** Consumes stdio / SSE / streamable-HTTP MCP servers *and* publishes its own toolbox at `/v1/mcp/serve`. External agents see what internal agents see. | First-class | Consumer only | Consumer only (v1.6+) | Limited / via plugins |
| **RAG with first-class citations.** `ContentBlock::Citation` is a typed variant — source URI, line span, preview, score. Adapters that can't cite must not silently emit unsourced text. | First-class | None native | Cited in UI, schema opaque | Cited; long-standing bugs (#12655, #20829) |

## 5-minute quickstart

```bash
git clone https://github.com/xiaoguai-agent/xiaoguai.git
cd xiaoguai
docker compose -f deploy/docker-compose.yml up --build
# wait ~2 min on first build, then open:
open http://localhost:8080/healthz   # → ok
```

That's a full stack — `xiaoguai-core` on `:8080`, Postgres 16, Valkey
8 — running on `MockBackend` so it's self-contained out of the box.
For the chat UI, real LLM providers, MCP server registration, and the
admin console, see [`docs/user-guide/quickstart.md`](docs/user-guide/quickstart.md).

### Install pre-built binaries

All Linux packages bundle the web UI — after install, open `http://<host>:7600/`
(chat) and `/admin/` (console).

| Platform | Command |
|---|---|
| Debian / Ubuntu (amd64) | Download `xiaoguai-cli_*_amd64.deb` from the [latest release](https://github.com/xiaoguai-agent/xiaoguai/releases/latest) and `sudo apt install ./xiaoguai-cli_*_amd64.deb` |
| RHEL / Fedora / Rocky (amd64) | Download `xiaoguai-cli-*.x86_64.rpm` from the same release and `sudo rpm -i xiaoguai-cli-*.x86_64.rpm` |
| Bare-metal tarball (amd64 / arm64, glibc 2.35+) | Download `xiaoguai-vX.Y.Z-<arch>-unknown-linux-gnu.tar.gz`, extract, and `sudo bash scripts/install.sh` (systemd) |
| Container / full stack | `docker compose -f deploy/docker-compose.yml up --build` |
| Build from source | `cargo install --path crates/xiaoguai-cli --locked` |

The sandboxed code-execution MCP server (`xiaoguai-mcp-exec`) builds from
this workspace: `cargo install --path crates/xiaoguai-mcp-exec --locked`.

After install, the canonical entrypoint is `xiaoguai serve`. The
`xiaoguai-core` shim from earlier versions still works (the .deb wires
it in for systemd backward-compat) but new operators should use the
unified CLI.

## Kubernetes observability (optional)

An optional Helm sub-chart at `deploy/helm/xiaoguai-observability/` bundles
Prometheus, Grafana, Loki, and Tempo with pre-provisioned datasources, the
Wave-3 overview dashboard, SLO alert rules, and a ServiceMonitor targeting
xiaoguai-core `/metrics`. All four components can be individually disabled.

```bash
helm repo add prometheus-community https://prometheus-community.github.io/helm-charts
helm repo add grafana https://grafana.github.io/helm-charts
helm dependency update deploy/helm/xiaoguai-observability
helm upgrade --install xiaoguai-obs deploy/helm/xiaoguai-observability \
  -f deploy/helm/xiaoguai-observability/values-dev.yaml \
  -n monitoring --create-namespace
```

See [`deploy/helm/xiaoguai-observability/README.md`](deploy/helm/xiaoguai-observability/README.md)
for dependency versions, persistent storage requirements, and production overrides.

## Architecture

Three layers, eighteen Rust crates, one workspace. Substrate at the
bottom is pure data + policy; domain crates in the middle implement
the agent + MCP + RAG + scheduler + eval primitives; edges at the top
are the protocols and binaries users actually touch.

```
edges      ┌──────────────┬──────────────┬──────────────┬──────────────┐
           │ xiaoguai-api │ xiaoguai-im- │ xiaoguai-cli │ xiaoguai-    │
           │ axum REST +  │ gateway      │ chat / eval  │ core         │
           │ SSE, 15+ /v1 │ + im-feishu  │ provider /   │ production   │
           │ endpoints    │ (+dingtalk / │ mcp / remote │ binary;      │
           │              │  wecom       │              │ wires all    │
           │              │  scaffolds)  │              │ crates       │
           └──────┬───────┴──────┬───────┴──────┬───────┴──────┬───────┘
                  │              │              │              │
domain     ┌──────┴──────────────┴──────────────┴──────────────┴───────┐
           │                                                            │
           │  xiaoguai-llm     LlmBackend + Ollama / OpenAI-compat /    │
           │                   Mock + LlmRouter + circuit breakers      │
           │  xiaoguai-mcp     stdio / SSE / streamable-HTTP clients +  │
           │                   McpSupervisor (live reload from DB)      │
           │  xiaoguai-agent   Toolbox + ReactAgent::run_stream +       │
           │                   AgentEvent + sliding-window history      │
           │  xiaoguai-rag     R2R HTTP + in-mem fallback + RagMcp-     │
           │                   Adapter + reindex_path                   │
           │  xiaoguai-        Trigger × RetryPolicy × JobRun +         │
           │   scheduler       FileWatch + Webhook + ProactiveChecker + │
           │                   BudgetLedger + 4 PushSinks + Pg repos    │
           │  xiaoguai-runtime run_to_completion / run_streamed /       │
           │                   run_to_sink — shared agent loop          │
           │  xiaoguai-eval    regression + capability suites +         │
           │                   5 graders + EvalRunner + CLI             │
           └──────┬─────────────────────────────────────────────────────┘
                  │
substrate  ┌──────┴─────────────────────────────────────────────────────┐
           │  xiaoguai-types   domain types + ID newtypes               │
           │  xiaoguai-config  Settings (server / db / cache / auth /   │
           │                   audit / scheduler / im / eval)           │
           │  xiaoguai-storage sqlx + Pg repos + Valkey cache + RLS-    │
           │                   aware migrations                         │
           │  xiaoguai-audit   ChainedAudit (HMAC) + PgAuditSink        │
           │  xiaoguai-auth    JwtValidator (RS256/ES256, JWKS cache)   │
           │                   + Casbin                                 │
           └────────────────────────────────────────────────────────────┘
```

For the long-form crate dependency rules and where to plug in a new
bridge (trait in `xiaoguai-api` or `xiaoguai-scheduler`, impl in
`xiaoguai-core::scheduler_bridge`), see
[`docs/HANDOFF-2026-05-24.md`](docs/HANDOFF-2026-05-24.md) §3.

## Status

v1 is feature-complete as of 2026-05-24. Thirteen tags landed in the
final sprint on top of v0.10.0; `cargo test --workspace` reports
**443 passed / 0 failed / 66 ignored**; clippy and fmt are clean.

| Tag | Headline | Plan doc |
|---|---|---|
| v0.10.1 | reactive triggers — FileWatch + Webhook + `JobRunner::run_loop` | [plan](docs/plans/2026-05-23-v0.10.1.md) |
| v0.6.5  | `PgAuditSink` bootstrap + audit chain verify endpoint + IM tenant routing | [plan](docs/plans/2026-05-23-v0.6.5.md) |
| v0.7.4  | IM gateway PG-history default + persisted tool turns + replay cap | [plan](docs/plans/2026-05-23-v0.7.4.md) |
| v0.9.4.1| `McpSupervisor` live-pickup on marketplace install | [plan](docs/plans/2026-05-23-v0.9.4.1.md) |
| v0.10.2 | proactive triggers — `ProactiveChecker` + budget + reason | [plan](docs/plans/2026-05-23-v0.10.2.md) |
| v0.10.3 | push sinks — Feishu / Telegram / Email / Inbox | [plan](docs/plans/2026-05-23-v0.10.3.md) |
| v0.8.3  | chat-ui code-block syntax highlighting + copy button | [plan](docs/plans/2026-05-23-v0.8.3.md) |
| v0.11.0 | `xiaoguai-eval` crate — regression + capability suites + graders + CLI | [plan](docs/plans/2026-05-23-v0.11.0.md) |
| v0.11.1 | audit-first console — Today view + `/v1/admin/today` endpoint | [plan](docs/plans/2026-05-23-v0.11.1.md) |
| v0.11.2 | eval pane — run suites + convert session to case | [plan](docs/plans/2026-05-23-v0.11.2.md) |
| v0.12.0 | `xiaoguai-runtime` + PG scheduler repos + operator wiring + webhook HTTP route | [plan](docs/plans/2026-05-24-v0.12.0.md) |
| v0.12.1 | natural-language job definition + per-run synthetic session | [plan](docs/plans/2026-05-24-v0.12.1.md) |
| v0.12.2 | file watcher RAG re-index wiring + Obsidian catalog entry | [plan](docs/plans/2026-05-24-v0.12.2.md) |

The full v0.9 → v0.12 master plan is at
[`docs/plans/2026-05-23-roadmap-v0.9-v0.12.md`](docs/plans/2026-05-23-roadmap-v0.9-v0.12.md).

## Compliance

Xiaoguai is built for self-hosted deployments that need to defend their
audit trail to a third party.

- **等保 2.0 Level 3 self-check (`三级`)** — control mapping at
  [`docs/compliance/dengbao-2.0-l3/`](docs/compliance/dengbao-2.0-l3/).
  Covers the mandatory items in GB/T 22239-2019; operators still run
  the formal graded assessment with an MPS-accredited assessor.
- **GDPR DPIA template** — pre-filled threat model and lawful-basis
  worksheet at
  [`docs/compliance/gdpr/dpia-template.md`](docs/compliance/gdpr/dpia-template.md).

Hard guarantees the platform enforces in code (not just docs):

- HMAC-chained `audit_log` rows for every tool call, scheduled run,
  and IM-routed message. Chain verification is exposed at
  `/v1/admin/audit/verify`.
- Postgres row-level security on every tenant-scoped table.
- OIDC RS256 / ES256 JWT validation with JWKS cache + Casbin RBAC.
- Per-user proactive-push budget with a mandatory `reason` field —
  sinks may refuse delivery if the reason is empty.

## Roadmap

**v1.0 — shipped.** Everything in the table above plus the full v0.1
→ v0.10.0 history. The repo is ready for first users.

**v1.1 — not yet queued.** The honest plan is *"wait for first-user
feedback, then prioritise."* The candidate backlog, per
[`docs/HANDOFF-2026-05-24.md`](docs/HANDOFF-2026-05-24.md) §5:

- Per-tenant API tokens for `/v1/admin/scheduler/webhooks/...`
  (GitHub / Slack integrators today need the admin bearer).
- `CompositeExecutor` so the scheduler operator can dispatch by
  payload kind instead of the current hard-coded
  `RuntimeJobExecutor`.
- Admin-ui Scheduler pane (backend ships, UI doesn't).
- `RagClient` binary-file re-index path (text-only today).
- `notify-debouncer-full` for the file-watch source.
- First-party write-capable Obsidian connector (community server is
  read-only).
- Browser-walked screenshots + per-pane visual QA on chat-ui and
  admin-ui — every UI-affecting tag from v0.8.1 onward was tuned by
  reading, not eyeballing.
- Conversation fork, public-cloud LLM provider configs, `/usage`
  endpoint, HA (PG replica + Valkey cluster), multi-agent
  orchestration — see the roadmap §3 v1.0+ section.

## License

Licensed under the [Business Source License 1.1](LICENSE).

- **Self-hosted production use:** free.
- **Embedding in your own products:** free, when agent capabilities
  aren't your primary feature.
- **Competing managed / SaaS offering:** requires a commercial
  license.
- **Change Date:** four years after each version's release date, the
  source converts to **Apache License 2.0**.

BUSL-1.1 is not an OSI-approved Open Source license; it is
*source available + eventually open*. The full text and the
"Additional Use Grant" carve-outs are in [`LICENSE`](LICENSE).

## Documentation

The full handbook is hosted at **<https://xiaoguai-agent.github.io/xiaoguai/>**.

Source lives in [`docs/book/`](docs/book/). To build locally:

```bash
# Install mdbook and mdbook-mermaid first
cargo install mdbook mdbook-mermaid
# Then:
bash docs/book/test-build.sh
```

---

## Wave-3 features (v1.2.x / v1.3.x)

Wave 3 merged 33 feature branches into `main` in late May 2026. The
workspace now passes **1,191 tests / 0 failed / 92 ignored**. Three
Postgres bridges are still wired to return `503` in production until
v1.3 lands — see the honest status section below.

### What shipped

| Feature | One-liner |
|---|---|
| **Human-on-the-Loop policy (HotL)** | Risk-tiered approval gates; every agent action with `risk ≥ threshold` pauses for a human `APPROVE` / `REJECT` before proceeding. |
| **Outcome telemetry & attribution** | Every agent action is recorded with `session_id + tool + latency + cost + outcome`; the chain reader exposes `/v1/outcomes/chain/{session_id}` for audit consumers. |
| **Skill packs** | Declarative install: `POST /v1/skills/install {"slug":"incident-triage"}` records the pack row; 7 packs ship in-repo (`ar-collections`, `incident-triage`, `pr-review`, `hr-onboarding`, `rag-legal`, `rag-finance`, `rag-hr`) with `catalog/skill_packs.json` as the authoritative manifest. |
| **Active watchers (`xiaoguai-watch`)** | New crate; SQL-poll and HTTP-poll wakeups that feed the scheduler, enabling reactive "check every N seconds, fire when condition changes" loops without a dedicated worker process. |
| **Anomaly detection (`xiaoguai-anomaly`)** | Z-score and EWMA detectors over any numeric time series; ships as a standalone crate consumable by scheduler jobs and HotL policy rules. |
| **Rate-limit** | Per-tenant, per-route token-bucket enforced at the Axum middleware layer; config lives in `0014_tenant_rate_limit.sql`. |
| **New IM adapters** | Discord (Ed25519 sig verification), Telegram (Bot API long-poll), Mattermost (WebSocket), Slack (HMAC sig verification) — four new `xiaoguai-im-*` crates alongside the existing Feishu / DingTalk / WeCom adapters. |
| **Cloud LLM v2** | `ProviderKind` gains `Bedrock` (SigV4), `AzureOpenAi`, `Mistral`, and `Groq` — all behind the existing `LlmBackend` trait; circuit breakers and cost-quota defence carry over automatically. |
| **Observability** | New `xiaoguai-observability` crate; opt-in Prometheus scrape endpoint (`/metrics`) and OTLP trace export; zero telemetry by default (ADR-0013 preserved). |

### Quickstart — wave-3 full stack

The base `docker-compose.yml` brings up `xiaoguai-core + postgres + valkey`.
Add the observability sidecar profile for the full wave-3 stack:

```yaml
# deploy/docker-compose.wave3.yml  (create or adapt from the snippet below)
services:
  otel-collector:
    image: otel/opentelemetry-collector-contrib:0.101.0
    command: ["--config=/etc/otel.yaml"]
    volumes: ["./observability/otel.yaml:/etc/otel.yaml:ro"]

  prometheus:
    image: prom/prometheus:v2.52.0
    volumes: ["./observability/prometheus.yml:/etc/prometheus/prometheus.yml:ro"]
    ports: ["9090:9090"]

  grafana:
    image: grafana/grafana:10.4.2
    environment: {GF_SECURITY_ADMIN_PASSWORD: xiaoguai}
    volumes:
      - "./observability/grafana/provisioning:/etc/grafana/provisioning:ro"
      - "./observability/grafana/dashboards:/var/lib/grafana/dashboards:ro"
    ports: ["3000:3000"]
```

```bash
# Bring up everything
docker compose -f deploy/docker-compose.yml \
               -f deploy/docker-compose.wave3.yml up --build

# Apply wave-3 migrations (run once, idempotent after)
docker compose exec xiaoguai-core xiaoguai migrate run
# Migrations that land new in wave 3:
#   0011_hotl_policies.sql
#   0012_outcomes.sql
#   0015_skill_packs.sql

# Seed the skill-pack catalog
curl -s -X POST http://localhost:7600/v1/admin/skills/seed \
     -H "Authorization: Bearer $ADMIN_TOKEN"

# Grafana → http://localhost:3000  (admin / xiaoguai)
# Prometheus → http://localhost:9090
```

The binary is `xiaoguai` — not `xg`. CLI subcommands for wave-3 features
(`skills`, `outcomes`, `hotl`) are planned but not yet wired; use the REST
API or the admin-ui in the meantime.

### Documentation index

#### Operator guides (mdbook)

| Chapter | Path |
|---|---|
| Active wakeup / watchers | `docs/book/src/operator/` — day2.md §"Reactive watcher" |
| HotL policy | pending — see `docs/plans/2026-05-24-v1.1.3.md` |
| Outcome telemetry | pending — see `docs/plans/2026-05-24-v1.1.4.md` |
| Skill packs | pending — see `docs/book/src/skills/overview.md` |

Build the handbook locally:

```bash
cargo install mdbook mdbook-mermaid
bash docs/book/test-build.sh
```

#### Runbooks

| Runbook | File |
|---|---|
| Observability (Prometheus + OTLP) | [`docs/runbooks/observability.md`](docs/runbooks/observability.md) |
| Operator day-2 | [`docs/runbooks/operator.md`](docs/runbooks/operator.md) |
| High availability | [`docs/runbooks/ha.md`](docs/runbooks/ha.md) |
| Kubernetes / Helm | [`docs/runbooks/k8s-helm.md`](docs/runbooks/k8s-helm.md) |
| AWS Terraform | [`docs/runbooks/aws-terraform.md`](docs/runbooks/aws-terraform.md) |
| Release signing | [`docs/runbooks/release-signing.md`](docs/runbooks/release-signing.md) |

#### Architecture

| Document | Path |
|---|---|
| ADR-0013 Zero-default telemetry | [`docs/architecture/adr/0013-zero-default-telemetry.md`](docs/architecture/adr/0013-zero-default-telemetry.md) |
| ADR-0014 Multimodal MCP architecture | [`docs/architecture/adr/0014-multimodal-mcp-architecture.md`](docs/architecture/adr/0014-multimodal-mcp-architecture.md) |
| ADR-0009 Cost quota + token-bomb defence | [`docs/architecture/adr/0009-cost-quota-and-token-bomb-defense.md`](docs/architecture/adr/0009-cost-quota-and-token-bomb-defense.md) |
| ADR-0008 Tool-result provenance | [`docs/architecture/adr/0008-tool-result-provenance.md`](docs/architecture/adr/0008-tool-result-provenance.md) |
| Multi-agent peer topology | [`docs/architecture/multi-agent-peer.md`](docs/architecture/multi-agent-peer.md) |
| System design (v0.1 origin) | [`docs/architecture/2026-05-21-design.md`](docs/architecture/2026-05-21-design.md) |

#### Compliance

Existing mappings cover 等保 2.0 L3 and GDPR (see the Compliance
section above). SOC 2, HIPAA, PCI-DSS, ISO 27001, and EU AI Act
control mappings are on the roadmap — not yet written.

#### API

The REST API surface (15+ endpoints) is described in
[`docs/book/src/api/rest.md`](docs/book/src/api/rest.md) and the MCP
toolbox in [`docs/book/src/api/mcp.md`](docs/book/src/api/mcp.md).
An OpenAPI spec and Bruno collection are planned for v1.3; the routes
are all typed in `crates/xiaoguai-api/src/routes/`.

#### Skill packs

| Resource | Path |
|---|---|
| Pack catalog (machine-readable) | [`catalog/skill_packs.json`](catalog/skill_packs.json) |
| AR Collections | [`packs/ar-collections/README.md`](packs/ar-collections/README.md) |
| Incident Triage | `packs/incident-triage/` |
| PR Review | `packs/pr-review/` |
| HR Onboarding | `packs/hr-onboarding/` |
| RAG — Legal | `packs/rag-legal/` |
| RAG — Finance | `packs/rag-finance/` |
| RAG — HR | `packs/rag-hr/` |

#### Recipes & examples

| Recipe | Path |
|---|---|
| Multi-agent peer pair | [`examples/multi-agent/peer-pair/README.md`](examples/multi-agent/peer-pair/README.md) |
| Grafana dashboard pack | [`observability/grafana/README.md`](observability/grafana/README.md) |

#### SDKs

| SDK | Status |
|---|---|
| Python (`xiaoguai` PyPI package) | Shipped — wraps the binary via subprocess; see `python/xiaoguai/` |
| TypeScript | Planned (v1.3) |
| Go | Planned (v1.4) |
| Java | Under consideration |

### Honest status — what is NOT production-ready yet

Three Postgres bridge implementations are stubbed and return `503` until
v1.3 wires the real implementations:

- **`/v1/hotl/*`** — HotL policy CRUD and approval-gate evaluation.
  `HotlPolicyStore` trait is defined; `AppState.hotl_policy_store` field
  exists; the Postgres bridge is pending.
- **`/v1/outcomes/*`** — Outcome recording and chain-reader queries.
  `OutcomeWriter` / `OutcomesReader` traits are defined; the Postgres bridge
  is pending.
- **`/v1/skills/*`** — Skill pack install, list, and uninstall.
  `0015_skill_packs.sql` migration is ready; the HTTP routes exist but the
  store bridge returns `503`.

The **pack runtime loader** is also not yet wired: installing a pack via
the API records the row in the `skill_packs` table but does not yet
activate the pack's prompt overlays or tool registrations at runtime.

Everything else in wave 3 — rate-limit middleware, observability, IM
adapters, cloud LLM providers, anomaly / watcher crates — is fully wired
and tested.

---

*Built in Shanghai. 2026.*
