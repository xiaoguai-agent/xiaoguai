# Architecture Overview

Xiaoguai is built as a Rust workspace of ~34 crates across three layers:
**substrate** (pure data + audit), **domain** (agent, MCP, RAG, scheduler, eval),
and **edges** (REST API, IM gateway, CLI, production binary).

> **Single-owner SQLite (DEC-033 / DEC-HLD-021).** Xiaoguai ships as a single
> binary over **embedded SQLite** with one implicit owner. There is no
> Postgres, no row-level security, no OIDC/JWT, no Casbin RBAC, and no
> multi-tenancy; the access gate is an optional HTTP Basic username/password.

## Layer diagram

```
edges      ┌──────────────┬──────────────┬──────────────┬──────────────┐
           │ xiaoguai-api │ xiaoguai-im- │ xiaoguai-cli │ xiaoguai-    │
           │ axum REST +  │ gateway      │ chat / eval  │ core         │
           │ SSE, 15+ /v1 │ + im-feishu  │ provider /   │ production   │
           │ endpoints    │ (+dingtalk / │ mcp / remote │ binary;      │
           │              │  wecom)      │              │ wires all    │
           └──────┬───────┴──────┬───────┴──────┬───────┴──────┬───────┘
                  │              │              │              │
domain     ┌──────┴──────────────┴──────────────┴──────────────┴───────┐
           │  xiaoguai-llm     LlmBackend + Ollama / OpenAI-compat /    │
           │                   Mock + LlmRouter + circuit breakers      │
           │  xiaoguai-mcp     stdio / SSE / streamable-HTTP clients +  │
           │                   McpSupervisor (live reload from DB)      │
           │  xiaoguai-agent   Toolbox + ReactAgent::run_stream +       │
           │                   AgentEvent + sliding-window history      │
           │  xiaoguai-rag     R2R HTTP + in-mem fallback + RagMcp-     │
           │                   Adapter + reindex_path                   │
           │  xiaoguai-scheduler  Trigger × RetryPolicy × JobRun +     │
           │                   FileWatch + Webhook + ProactiveChecker + │
           │                   BudgetLedger + 4 PushSinks + SQLite repos│
           │  xiaoguai-runtime run_to_completion / run_streamed /       │
           │                   run_to_sink — shared agent loop          │
           │  xiaoguai-eval    regression + capability suites +         │
           │                   5 graders + EvalRunner + CLI             │
           └──────┬─────────────────────────────────────────────────────┘
                  │
substrate  ┌──────┴─────────────────────────────────────────────────────┐
           │  xiaoguai-types   canonical types (ContentBlock, AgentEvent,│
           │                   Citation, ToolCall, …)                   │
           │  xiaoguai-audit   append-only HMAC-chained AuditLog trait  │
           │  xiaoguai-auth    HotL argument redaction (JSONPath rules) │
           │  xiaoguai-storage embedded SQLite migrations (sqlx) +      │
           │                   in-process cache (DashMap)       │
           └─────────────────────────────────────────────────────────────┘
```

## Key design decisions

### Audit-first

Every operation — chat turn, tool call, scheduled job, IM message — writes
an HMAC-chained audit row before the response is sent. The chain is
verifiable offline: `xiaoguai admin audit verify`.

### MCP two-way

Xiaoguai is simultaneously an MCP **consumer** (connecting to external MCP servers
via `McpSupervisor`) and an MCP **publisher** (exposing its own
`Toolbox` at `GET /v1/mcp/serve`). External agents and peer xiaoguai instances
both connect over Streamable-HTTP.

### ReAct loop, not workflow editor

The agent loop is a pure ReAct loop: observe → reason → act, repeated until
the model produces a final answer or the budget is exhausted. There is no
drag-and-drop workflow editor. Complexity lives in composing MCP tools, not
in platform-specific pipeline DSLs.

### Local-LLM default

`LlmRouter` selects among registered providers by model name. The default
compose stack ships with `MockBackend` so the platform runs without any
external LLM. Connecting Ollama or any OpenAI-compatible endpoint requires
one `xiaoguai provider register` command.

## Storage

| Component | Role |
|-----------|------|
| **Embedded SQLite** | Sessions, messages, MCP registry, LLM providers, scheduled jobs, audit log — the single bundled store; no external DB server |
| **In-process cache** | Idempotency keys + short-lived caches in a process-local `DashMap` — no Valkey/Redis sidecar |

## Delivery paths

| Path | Command |
|------|---------|
| docker-compose | `docker compose -f deploy/docker-compose.yml up` |
| Native package | install the release `.deb` / `.rpm` (bundles the web UI) |
| Bare-metal tarball | `curl … | tar xz` then run the binary / install the systemd unit |
| pip wheel | `pip install xiaoguai && xiaoguai serve` |

## Further reading

- [Multi-Agent Peer Topology](architecture/multi-agent.md) — how peer MCP links compose xiaoguai instances
- [Crate Layout](architecture/crates.md) — full workspace inventory
