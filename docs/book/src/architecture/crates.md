# Crate Layout

The workspace lives in `crates/`. Each crate has a single stated responsibility.

## Substrate layer (no runtime deps, pure data + policy)

| Crate | Purpose |
|-------|---------|
| `xiaoguai-types` | Canonical types: `ContentBlock`, `AgentEvent`, `Citation`, `ToolCall`, `LlmResponse`, … |
| `xiaoguai-audit` | Append-only `AuditLog` trait + HMAC-chain verification |
| `xiaoguai-policy` | Casbin RBAC model + OIDC RS256/ES256 JWT validation with JWKS cache |
| `xiaoguai-storage` | `sqlx` migrations runner (0001–0009), Valkey client helpers |

## Domain layer (business logic)

| Crate | Purpose |
|-------|---------|
| `xiaoguai-llm` | `LlmBackend` trait + Ollama / OpenAI-compat / Mock impls + `LlmRouter` + circuit breakers |
| `xiaoguai-mcp` | stdio / SSE / Streamable-HTTP MCP clients + `McpSupervisor` (live reload from DB) |
| `xiaoguai-agent` | `Toolbox` + `ReactAgent::run_stream` + `AgentEvent` + sliding-window history |
| `xiaoguai-rag` | R2R HTTP client + in-memory fallback + `RagMcpAdapter` + `reindex_path` |
| `xiaoguai-scheduler` | `Trigger` × `RetryPolicy` × `JobRun` + FileWatch + Webhook + `ProactiveChecker` + `BudgetLedger` + `CompositeExecutor` + PG repos |
| `xiaoguai-runtime` | `run_to_completion` / `run_streamed` / `run_to_sink` — the shared agent loop |
| `xiaoguai-eval` | Regression + capability suites + 5 graders + `EvalRunner` + CLI |

## Edge layer (protocols and user-facing binaries)

| Crate | Purpose |
|-------|---------|
| `xiaoguai-api` | axum REST + SSE (15+ `/v1` endpoints), `AppState`, all route handlers |
| `xiaoguai-im-gateway` | IM ingress router (delegates to per-platform adapters) |
| `xiaoguai-im-feishu` | Feishu webhook signature verify + OpenAPI reply + token cache |
| `xiaoguai-im-dingtalk` | DingTalk signature verify + OpenAPI reply + token cache (v1.1.3) |
| `xiaoguai-im-wecom` | WeCom signature verify + XML parser + OpenAPI reply + token cache (v1.1.3) |
| `xiaoguai-cli` | `chat`, `eval`, `provider`, `mcp`, `remote` sub-commands |
| `xiaoguai-core` | Production binary — wires all crates into one process |

## Bridge layer (core's adapter impls)

`xiaoguai-core` also hosts `*_bridge` modules that implement the API-layer traits against real PG/Valkey:

- `usage_bridge`, `sessions_bridge` — `/v1/usage`, `/v1/sessions/:id/fork`
- `scheduler_bridge` — ~10 adapters including `LlmNlJobCompiler`, `PgWebhookTokenValidator`, `RagReindexExecutor`, `spawn_file_watch_source`

## PG migrations

| Migration | Content |
|-----------|---------|
| 0001 | Initial schema (tenants, users, sessions, messages) |
| 0002 | MCP registry |
| 0003 | LLM providers |
| 0004 | Audit log |
| 0005 | Scheduled jobs |
| 0006 | Budget ledger |
| 0007 | Eval results |
| 0008 | Session fork (`parent_session_id`) |
| 0009 | Scheduler webhook tokens |
