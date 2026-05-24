# REST API Reference

> **Placeholder** — full OpenAPI spec forthcoming. The endpoints below are complete as of v1.1.

## Base URL

```
http://localhost:8080/v1
```

All endpoints return `application/json`. Streaming endpoints return `text/event-stream` (SSE).

## Authentication

Set `Authorization: Bearer <jwt>` when `XIAOGUAI_AUTH_REQUIRED=true`. In development, auth is
disabled by default.

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/healthz` | Liveness probe |
| `GET` | `/metrics` | Prometheus metrics |
| `POST` | `/v1/sessions` | Create a session |
| `GET` | `/v1/sessions/:id` | Get session metadata |
| `POST` | `/v1/sessions/:id/messages` | Send a message (SSE response) |
| `GET` | `/v1/sessions/:id/messages` | List messages |
| `POST` | `/v1/sessions/:id/cancel` | Cancel a running session |
| `POST` | `/v1/sessions/:id/fork` | Fork a session from a message ID |
| `GET` | `/v1/usage` | Usage summary (24h + rolling) |
| `GET` | `/v1/mcp/serve` | MCP Streamable-HTTP server |
| `POST` | `/v1/im/feishu/webhook` | Feishu inbound webhook |
| `POST` | `/v1/im/dingtalk/webhook` | DingTalk inbound webhook |
| `POST` | `/v1/im/wecom/webhook` | WeCom inbound webhook |
| `GET/POST` | `/v1/scheduler/webhooks/:route_id` | Scheduler webhook trigger |
| `GET/POST/PUT/DELETE` | `/v1/admin/**` | Admin endpoints (jobs, providers, MCP, tokens) |

## SSE event types

| Event | Payload |
|-------|---------|
| `text_delta` | `{"text": "…"}` |
| `tool_call_start` | `{"id": "…", "name": "…", "input": {…}}` |
| `tool_call_result` | `{"id": "…", "content": […]}` |
| `done` | `{"session_id": "…", "turn_id": "…"}` |
| `error` | `{"code": "…", "message": "…"}` |

## OpenAPI spec

> Full spec generation from axum routes is tracked as a v1.2 item.
> Until then use the endpoint table above or read `crates/xiaoguai-api/src/routes/` directly.
