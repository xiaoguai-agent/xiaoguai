# Crate Layout

The workspace lives in `crates/` (~34 crates). Each crate has a single stated
responsibility.

> **Single-owner SQLite (DEC-033 / DEC-HLD-021).** Storage is embedded SQLite;
> there is no `xiaoguai-policy` crate, no Postgres, no OIDC/Casbin, and no
> tenant axis. The optional access gate lives in `xiaoguai-api` (HTTP Basic);
> `xiaoguai-auth` is now only the HotL redaction engine.

## Substrate layer (pure data + audit)

| Crate | Purpose |
|-------|---------|
| `xiaoguai-types` | Canonical types: `ContentBlock`, `AgentEvent`, `Citation`, `ToolCall`, `Session`, `UserId`, … |
| `xiaoguai-config` | Layered configuration (YAML + `XIAOGUAI_*` env overrides) |
| `xiaoguai-audit` | Append-only `AuditLog` trait + HMAC-chain build/verify + redact + export |
| `xiaoguai-auth` | HotL argument redaction (`RedactionRules`, JSONPath) — DEC-HLD-014 |
| `xiaoguai-storage` | `sqlx` embedded-SQLite migrations runner + repositories + optional Valkey/in-process cache |

## Domain layer (business logic)

| Crate | Purpose |
|-------|---------|
| `xiaoguai-llm` | `LlmBackend` trait + Ollama / OpenAI-compat / Mock impls + `LlmRouter` + circuit breakers |
| `xiaoguai-mcp` | stdio / SSE / Streamable-HTTP MCP clients + `McpSupervisor` (live reload) + OAuth2-PKCE token store |
| `xiaoguai-mcp-exec` / `-exec-js` / `-exec-wasm` | Sandboxed MCP tool execution (native / JS / WASM tiers) |
| `xiaoguai-agent` | `Toolbox` + `ReactAgent::run_stream` + `AgentEvent` + sliding-window history + HotL gate |
| `xiaoguai-orchestrator` | Agent registry + capability router + conflict arbitration (multi-agent peer topology) |
| `xiaoguai-rag` | R2R HTTP client + in-memory fallback + `RagMcpAdapter` + `reindex_path` |
| `xiaoguai-memory` | Long-term memory store + embedding-backed recall |
| `xiaoguai-scheduler` | `Trigger` × `RetryPolicy` × `JobRun` + FileWatch + Webhook + `ProactiveChecker` + `BudgetLedger` + `CompositeExecutor` + SQLite repos |
| `xiaoguai-runtime` | `run_to_completion` / `run_streamed` / `run_to_sink` — the shared agent loop |
| `xiaoguai-tasks` | Skill-authoring proposals + skill-pack tasks |
| `xiaoguai-personas` | Persona definitions + CRUD |
| `xiaoguai-watch` | Watch DSL — SQL-backed alert sources + dedup |
| `xiaoguai-anomaly` | Anomaly detection over outcome/metric series |
| `xiaoguai-eval` | Regression + capability suites + graders + `EvalRunner` |
| `xiaoguai-observability` | Tracing / metrics / OTel wiring |

## Edge layer (protocols and user-facing binaries)

| Crate | Purpose |
|-------|---------|
| `xiaoguai-api` | axum REST + SSE (`/v1` endpoints), `AppState`, route handlers, HTTP Basic auth gate |
| `xiaoguai-im-gateway` | IM ingress router (delegates to per-platform adapters) |
| `xiaoguai-im-feishu` / `-dingtalk` / `-wecom` | Chinese IM adapters: signature verify + OpenAPI reply + token cache |
| `xiaoguai-im-slack` / `-discord` / `-telegram` / `-mattermost` | Additional IM adapters |
| `xiaoguai-cli` | `chat`, `eval`, `provider`, `mcp`, `remote`, `serve`, … sub-commands |
| `xiaoguai-core` | Production binary — wires all crates into one process |
| `xiaoguai-migrate-smoke` | CI smoke test asserting the embedded-SQLite migration set applies cleanly |

## Bridge layer (core's adapter impls)

`xiaoguai-core` hosts `*_bridge` modules that implement the API-layer traits
against the real SQLite-backed repositories:

- `usage_bridge`, `sessions_bridge` — `/v1/usage`, `/v1/sessions/:id/fork`
- `scheduler_bridge` — adapters including `LlmNlJobCompiler`, the webhook token validator, `RagReindexExecutor`, `spawn_file_watch_source`
- `audit_bridge`, `hotl_bridge`, `skills_bridge`, `memory`/`outcomes`/`today`/`eval` bridges

## Migrations

The embedded-SQLite schema is built by `sqlx` migrations
`0001_initial.sql … 0028_llm_provider_api_key.sql` under
`crates/xiaoguai-storage/migrations/`. They cover sessions/messages, the MCP
registry, LLM providers, the audit log, scheduled jobs + webhook tokens, the
budget ledger, eval results, session fork, HotL escalations/policies, skill
packs/proposals, memory, personas, outcomes, and watch state. The `tenants`
table and all `tenant_id` columns were dropped under the single-user pivot.
