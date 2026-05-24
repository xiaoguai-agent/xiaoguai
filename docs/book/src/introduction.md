# Introduction

**Xiaoguai (小怪)** is a self-hostable AI agent platform for technical individuals,
small teams, and anyone with a compliance or traceability constraint.

> *Your Little Agent for Big Work · 小怪不小，能办大事*

## Why Xiaoguai?

| Capability | Xiaoguai | n8n | Dify | OpenWebUI |
|---|---|---|---|---|
| **Audit-first console** — every chat, IM message, and scheduled run carries HMAC-chained audit metadata | First-class | Workflow runs only | Workflow runs only | Not surfaced |
| **Scheduler-native** — cron + file watcher + webhook + LLM-initiated runs with per-user budget | First-class | Strong triggers, weak agents | Cron only | None |
| **MCP two-way** — consumes *and* publishes MCP servers via `/v1/mcp/serve` | First-class | Consumer only | Consumer only (v1.6+) | Limited |
| **First-class citations** — `ContentBlock::Citation` is a typed variant with source URI + line span | First-class | None native | UI only | Bug-prone |

## Key properties

- **Single Rust binary** — `xiaoguai-core` plus Postgres 16 and Valkey 8. No Python runtime, no JVM, no JS server on the hot path.
- **Local-LLM default** — Ollama and any OpenAI-compatible endpoint work out of the box. Public-cloud LLM providers are optional.
- **Multi-tenant with RLS** — Postgres row-level security on every tenant-scoped table; Casbin RBAC with OIDC JWT.
- **Chinese IM native** — Feishu (飞书), DingTalk (钉钉), and WeCom (企微) adapters ship in the box.
- **四路交付** — docker-compose, Helm chart, bare-metal tarball, and pip wheel from the same release.

## Getting started

Jump to the [Quickstart](quickstart.md) to have a running stack in five minutes.

For day-2 operations see the [Operator Guide](operator/overview.md).

For architecture details see [Architecture Overview](architecture.md).

---

> **Documentation hosted at:** <https://xiaoguai-agent.github.io/xiaoguai/>  
> Source: `docs/book/` in the [GitHub repository](https://github.com/xiaoguai-agent/xiaoguai)
