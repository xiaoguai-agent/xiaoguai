# Roadmap

> **Superseded by DEC-033 (single-user SQLite pivot).** This roadmap is kept
> as a record of intent, but the architecture direction changed: Xiaoguai now
> ships as a single binary over **embedded SQLite** with a single owner. The
> Postgres / OIDC-JWT / Casbin-RBAC / row-level-security / Helm / HA items
> below were **removed**, not shipped — auth is an optional HTTP Basic
> username/password and there is no multi-tenancy. See the
> [Architecture Overview](architecture.md).

## v1.0 — shipped

Everything needed for production single-node deployments:

- Rust workspace (now ~34 crates)
- ReAct agent loop with MCP two-way (consume + publish)
- Feishu IM adapter
- Single-owner access gate (optional HTTP Basic) — *(originally planned as
  OIDC/Casbin/RLS; removed under DEC-033)*
- HMAC-chained audit log
- Delivery paths: docker-compose, native `.deb`/`.rpm`, bare-metal tarball, pip wheel
- Regression eval framework

## v1.1 — shipped

Incremental shipping-readiness and product features:

| Tag | Feature |
|-----|---------|
| v1.1.1 | `/v1/usage` endpoint + Usage admin pane + Today 24h summary |
| v1.1.2 | Conversation fork (`POST /v1/sessions/:id/fork` + Branch button) |
| v1.1.3 | DingTalk + WeCom IM adapters (+37 tests) |
| v1.1.4 | HA scaffold — PG logical replication + Valkey Cluster + nginx |
| v1.1.5a | Multi-agent peer MVP (MCP peer links, integration test) |
| v1.1.5b | Supervisor plan doc |
| v1.1.6 | Bare-metal tarball + hardened systemd unit |
| v1.1.7 | pip wheel + cibuildwheel matrix |
| v1.1.8 | CI security — cargo-deny + cargo-audit cron + Snyk |
| v0.12.x.1 | Per-tenant webhook tokens + CompositeExecutor + Scheduler admin pane |

## v1.1.x follow-ups (deferred, tracked)

- Cost columns on `llm_providers` (unblocks Usage pane cost display)
- WeCom `EncodingAESKey` encrypted payload
- DingTalk Stream API long-poll client
- Replica-aware read pool routing (v1.1.4.1)
- Valkey `ClusterClient` migration (v1.1.4.x)
- cargo-vet bootstrap to blocking (v1.1.8.1)
- Dependabot auto-merge gates (v1.1.8.2)

## v1.2 — planned (wait for first user feedback)

- Supervisor / orchestrator crate (`xiaoguai-orchestrator`)
- PyO3 native Python bindings
- Real cargo-vet supply chain attestation (blocking gate)
- Multi-region deployment support
- rustdoc CI artifact alongside this handbook
- Public-cloud LLM providers (Anthropic, Gemini, 通义, DeepSeek, 智谱)

## v1.4 — candidates (ADR-0019)

- **Task Board** (`xiaoguai-tasks` crate) — durable multi-agent Kanban queue
  - Columns: TRIAGE / TO-DO / READY / RUNNING / BLOCKED / DONE
  - Configurable dispatch policy (FIFO / priority / round-robin) + pool sizing
  - Multi-board per tenant; HotL approval integration; outcome-telemetry attribution chain
  - Admin UI swimlane view + `xg tasks` CLI; REST `/v1/boards` + `/v1/boards/{id}/cards`
  - Pack integration: devops-oncall + incident-triage auto-create TRIAGE cards
  - See ADR-0019 for open questions (workspace scoping, auto-triage, card expiry, affinity fallback)

## Stability policy

- **Patch** (x.y.Z) — bug fixes, documentation, CI
- **Minor** (x.Y.0) — new features, backwards-compatible API additions
- **Major** (X.0.0) — breaking API changes (none planned before v2.0)

The REST API at `/v1/**` is stable as of v1.0. The internal crate APIs (`xiaoguai-*`) 
are semver-exempt until v2.0.
