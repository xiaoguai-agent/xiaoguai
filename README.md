# Xiaoguai 小怪

> Your Little Agent for Big Work · 小怪不小，能办大事

**Xiaoguai** is a lightweight, MCP-first, self-hostable AI agent platform designed for enterprise and private deployment.

```
┌─────────────────────────────────────────────────────────┐
│  Lightweight    Single Rust binary, <200MB image        │
│  MCP-First      Per-tenant MCP supervisor pool          │
│  Local LLM      Ollama / vLLM / OpenAI-compatible       │
│  Native IM      Feishu, DingTalk, WeCom (v1.1)          │
│  Compliant      OIDC + RBAC + audit hmac + 等保 + GDPR  │
└─────────────────────────────────────────────────────────┘
```

## Status

**Pre-v0.1** — skeleton in place, implementation pending. See [Roadmap](docs/architecture/2026-05-21-design.md#15-roadmap).

## Quick links

- [Design document](docs/architecture/2026-05-21-design.md) — canonical source for v1.0 decisions
- [Documentation](docs/) — everything lives under `docs/`
- [License (BSL 1.1)](LICENSE) — converts to Apache 2.0 after 4 years

## Repository layout

```
.
├── crates/                Rust workspace (14 crates, xiaoguai-* prefix)
├── proto/                 protobuf definitions (gRPC contracts)
├── frontend/              pnpm workspace: chat-ui + admin-ui + shared
├── deploy/                Helm chart, docker-compose, bare-metal scripts
├── docs/                  All documentation (only docs here, never in repo root)
├── examples/              MCP server demos, config samples
├── scripts/               Release scripts, SBOM gen, mirror seeding
└── .github/workflows/     CI: build / test / release / SBOM / cosign
```

## License

Licensed under the [Business Source License 1.1](LICENSE).

- **Self-hosted production use**: free
- **Embedding in your own products**: free (when agent capabilities aren't your primary feature)
- **Competing managed/SaaS offering**: requires commercial license
- **Change Date**: Four years after each version's release date, converts to **Apache License 2.0**

This is not an OSI-approved Open Source license. It is "source available + eventually open."

## Contributing

Pre-v0.1 — contribution process not yet finalized. See [docs/developer-guide/](docs/developer-guide/) once published.

---

*Built in Shanghai. 2026.*
