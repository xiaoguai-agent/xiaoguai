# Quickstart

This walks through bringing Xiaoguai up locally and sending your first
streaming chat message. Total time: ~5 minutes assuming Docker is
installed.

## 1. Bring up the stack

```bash
git clone https://github.com/xiaoguai-agent/xiaoguai.git
cd xiaoguai
docker compose -f deploy/docker-compose.yml up --build
```

Compose brings up Postgres 16, Valkey 8, and `xiaoguai-core` on
`http://localhost:8080`. The first build takes ~2 min; subsequent runs are
cached.

## 2. Confirm it's alive

```bash
curl http://localhost:8080/healthz       # → ok
```

## 3. Send your first chat

Using the bundled CLI (requires a local Rust toolchain — `cargo` is
enough):

```bash
cargo run -p xiaoguai-cli -- remote \
  --server http://localhost:8080 \
  chat \
  --user-id usr_dev --tenant-id ten_dev \
  --model mock \
  --prompt 'hello!'
```

The CLI:

1. POSTs to `/v1/sessions` to create a session.
2. POSTs the prompt to `/v1/sessions/:id/messages` which returns SSE.
3. Streams `text_delta` events to stdout, `tool_call_*` events to stderr,
   and a final `done` line.

Or directly via curl:

```bash
SID=$(curl -sX POST http://localhost:8080/v1/sessions \
  -H 'content-type: application/json' \
  -d '{"user_id":"usr_dev","tenant_id":"ten_dev","model":"mock"}' \
  | jq -r .id)

curl -N -X POST http://localhost:8080/v1/sessions/$SID/messages \
  -H 'content-type: application/json' \
  -d '{"content":"hello"}'
```

## 4. Browse the message history

```bash
curl http://localhost:8080/v1/sessions/$SID/messages | jq
```

## 5. Use the web UI

```bash
cd frontend
pnpm install
pnpm -F @xiaoguai/chat-ui dev
# → http://localhost:5173
```

The chat UI proxies `/v1` to `http://localhost:8080` so it picks up the
same compose stack.

## What ships in the v1.0 compose stack

- **xiaoguai-core** — REST + SSE API on :8080.
  - In v1.0 the default LLM backend is `MockBackend` so the stack runs
    self-contained. To use a real backend register a provider in PG via
    `xiaoguai provider register ...` and restart core — provider
    auto-selection on boot lands in v1.1.
- **postgres 16** — sessions / messages / mcp / providers / audit log.
- **valkey 8** — cache + idempotency keys.

Optional: uncomment the Ollama service in `deploy/docker-compose.yml`
and `docker compose exec ollama ollama pull qwen2.5-coder` for a local
LLM.

## Next steps

- Wire your own MCP server: `xiaoguai mcp register --name fs
  --transport stdio --command npx --args
  '-y,@modelcontextprotocol/server-filesystem,/workspace'`.
- Register a real LLM provider: `xiaoguai provider register --name
  deepseek --kind openai_compat --endpoint https://api.deepseek.com/v1
  --api-key-env DEEPSEEK_API_KEY --models deepseek-chat`.
- Deploy to Kubernetes via `deploy/helm/xiaoguai/` (see
  `deploy/helm/xiaoguai/values.yaml` for the secret refs the chart
  expects).
- For Feishu integration, configure the webhook URL to
  `https://your-host/v1/im/feishu/webhook` and the encrypt key into
  `XIAOGUAI_IM_FEISHU__ENCRYPT_KEY` (v0.7.1 will surface the
  configuration through `Settings`).

## Troubleshooting

| Symptom                               | Likely cause                                   |
|---------------------------------------|------------------------------------------------|
| `healthz` returns nothing             | Postgres not up — check `docker compose logs postgres`. |
| `POST /v1/sessions` returns 500       | Migrations haven't run — `docker compose restart xiaoguai-core` reruns them. |
| SSE stream stays empty                | MockBackend is configured to respond with a fixed string; for richer output configure a real provider. |
| 401 on `/v1/**`                       | `XIAOGUAI_AUTH_REQUIRED=true` was set without a valid JWT issuer. Unset it for dev or pass `Authorization: Bearer ...`. |
