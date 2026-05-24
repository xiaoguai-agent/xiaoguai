# Xiaoguai 小怪

> **Rust-implemented, audit-first, scheduler-native local agent platform.**
>
> *Your Little Agent for Big Work · 小怪不小，能办大事*

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

---

*Built in Shanghai. 2026.*
