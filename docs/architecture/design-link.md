# Design documents

The retrofit design documents for xiaoguai live in the sibling repo
`xiaoguai-agent-design` (separate from this implementation repo so the
two evolve independently — the design doc set is the *contract*, the
implementation is the *artifact*).

## Top-level documents

| Doc | Path | Status |
|---|---|---|
| Architecture philosophy (R.E.S.T model, REPL-container abstraction, control/data plane split) | [`harness-engineering.md`](../../xiaoguai-agent-design/docs/harness-engineering.md) | v1.4 |
| Product requirements | [`prd-xiaoguai.md`](../../xiaoguai-agent-design/docs/prd-xiaoguai.md) | v1.4 retrofit |
| High-level design | [`hld.md`](../../xiaoguai-agent-design/docs/hld.md) | v1.4 retrofit |
| API contract | [`api-contract.md`](../../xiaoguai-agent-design/docs/api-contract.md) | v1.4 retrofit |
| Project guardrails (license, MSRV, security defaults, CI) | [`guardrails.md`](../../xiaoguai-agent-design/docs/guardrails.md) | v1.4 retrofit |
| Test spec | [`test-spec.md`](../../xiaoguai-agent-design/docs/test-spec.md) | v1.4 retrofit |
| Test strategy | [`test-strategy.md`](../../xiaoguai-agent-design/docs/test-strategy.md) | v1.4 retrofit |
| Operator runbook | [`runbook.md`](../../xiaoguai-agent-design/docs/runbook.md) | v1.4 retrofit |

## Low-level design (per-component)

The `lld/` directory contains one doc per major crate or subsystem:

- [`lld-agent.md`](../../xiaoguai-agent-design/docs/lld/lld-agent.md) — ReAct loop, HotL gate, dispatch
- [`lld-llm.md`](../../xiaoguai-agent-design/docs/lld/lld-llm.md) — backends, router, breakers
- [`lld-mcp.md`](../../xiaoguai-agent-design/docs/lld/lld-mcp.md) — MCP client + supervisor
- [`lld-mcp-exec.md`](../../xiaoguai-agent-design/docs/lld/lld-mcp-exec.md) — Python sandbox MCP server
- [`lld-rag.md`](../../xiaoguai-agent-design/docs/lld/lld-rag.md) — RAG + citations
- [`lld-memory.md`](../../xiaoguai-agent-design/docs/lld/lld-memory.md) — pgvector store + embedder
- [`lld-audit.md`](../../xiaoguai-agent-design/docs/lld/lld-audit.md) — HMAC chain
- [`lld-storage.md`](../../xiaoguai-agent-design/docs/lld/lld-storage.md) — Postgres + Valkey + cache fallback
- [`lld-runtime.md`](../../xiaoguai-agent-design/docs/lld/lld-runtime.md) — shared loop drivers
- [`lld-orchestrator.md`](../../xiaoguai-agent-design/docs/lld/lld-orchestrator.md) — supervisor, multi-agent
- [`lld-personas.md`](../../xiaoguai-agent-design/docs/lld/lld-personas.md) — role presets
- [`lld-im-gateway.md`](../../xiaoguai-agent-design/docs/lld/lld-im-gateway.md) — 7 IM adapters
- See [`lld/index.md`](../../xiaoguai-agent-design/docs/lld/index.md) for the full list.

## Architectural decision records

Live in this implementation repo (not the design repo) so they're
versioned alongside the code: [`docs/architecture/adr/`](adr/).

## Relationship summary

```
implementation repo (xiaoguai)              design repo (xiaoguai-agent-design)
├── crates/                                  ├── docs/
├── docs/                                    │   ├── harness-engineering.md  ← philosophy
│   ├── architecture/                        │   ├── prd-xiaoguai.md         ← PRD
│   │   ├── adr/         ← decisions live    │   ├── hld.md                  ← HLD
│   │   └── design-link.md ← THIS file       │   ├── api-contract.md
│   ├── runbooks/        ← ops procedures    │   ├── guardrails.md
│   ├── plans/           ← session plans     │   ├── test-spec.md
│   └── HANDOFF-*.md     ← session deltas    │   ├── test-strategy.md
└── ...                                      │   ├── runbook.md
                                             │   └── lld/             ← per-component
                                             └── HANDOFF-DESIGN-UPDATE.md
```

If a doc above is missing or stale, see
[`docs/plans/2026-05-28-retro-design-docs.md`](../plans/2026-05-28-retro-design-docs.md)
— it tracks the update pass and what's been done vs. what's pending.
