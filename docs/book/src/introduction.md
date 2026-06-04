# Introduction

**Xiaoguai (小怪)** is a self-hostable AI agent platform for technical individuals,
small teams, and anyone with a compliance or traceability constraint.

> *Your Little Agent for Big Work · 小怪不小，能办大事*

## Why Xiaoguai?

| Capability | Xiaoguai | n8n | Dify | OpenWebUI |
|---|---|---|---|---|
| **Audit-first console** — every chat, IM message, and scheduled run carries HMAC-chained audit metadata | First-class | Workflow runs only | Workflow runs only | Not surfaced |
| **Scheduler-native** — cron + file watcher + webhook + LLM-initiated runs with budget gating | First-class | Strong triggers, weak agents | Cron only | None |
| **MCP two-way** — consumes *and* publishes MCP servers via `/v1/mcp/serve` | First-class | Consumer only | Consumer only (v1.6+) | Limited |
| **First-class citations** — `ContentBlock::Citation` is a typed variant with source URI + line span | First-class | None native | UI only | Bug-prone |

## Key properties

- **Single Rust binary** — `xiaoguai-core` with **embedded SQLite**. No external database server, no Python runtime, no JVM, no JS server on the hot path. An optional Valkey/Redis can back the cache; with none configured it falls back to an in-process cache.
- **Local-LLM default** — Ollama and any OpenAI-compatible endpoint work out of the box. Public-cloud LLM providers are optional.
- **Single-owner by design** — one implicit owner; an optional HTTP Basic username + password gates the API (leave it empty for an open localhost run). No OIDC, no JWT, no Casbin, no multi-tenancy.
- **Chinese IM native** — Feishu (飞书), DingTalk (钉钉), and WeCom (企微) adapters ship in the box.
- **多路交付** — docker-compose, native `.deb`/`.rpm`, bare-metal tarball, and pip wheel from the same release (all bundle the web UI).

## Getting started

Jump to the [Quickstart](quickstart.md) to have a running stack in five minutes.

For day-2 operations see the [Operator Guide](operator/overview.md).

For architecture details see [Architecture Overview](architecture.md).

---

> **Documentation source:** `docs/book/` in the [GitHub repository](https://github.com/xiaoguai-agent/xiaoguai).
> Build it locally with `mdbook build docs/book` (output under `docs/book/book/html`).
