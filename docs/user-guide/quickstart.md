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

Compose brings up a single `xiaoguai-core` container on
`http://localhost:7600`. State lives in **embedded SQLite** inside the
`xiaoguai_data` volume — there is no separate database or cache server to
run. The first build takes ~2 min; subsequent runs are cached.

> Prefer no Docker? Install a release `.deb`/`.rpm`/tarball (see the
> [Operator Guide](operator/overview.md)) or run from source with
> `cargo run -p xiaoguai-core` — it creates `./data.db` and serves on
> `:7600` with no other dependencies.

## 2. Confirm it's alive

```bash
curl http://localhost:7600/healthz       # → ok
```

## 3. Send your first chat

Using the bundled CLI (requires a local Rust toolchain — `cargo` is
enough):

```bash
cargo run -p xiaoguai-cli -- remote \
  --server http://localhost:7600 \
  chat \
  --user-id usr_dev \
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
SID=$(curl -sX POST http://localhost:7600/v1/sessions \
  -H 'content-type: application/json' \
  -d '{"user_id":"usr_dev","model":"mock"}' \
  | jq -r .id)

curl -N -X POST http://localhost:7600/v1/sessions/$SID/messages \
  -H 'content-type: application/json' \
  -d '{"content":"hello"}'
```

> If you set `auth.username` / `auth.password` (or the matching
> `XIAOGUAI_AUTH__USERNAME` / `XIAOGUAI_AUTH__PASSWORD` env vars), add
> `-u "$USER:$PASS"` to every curl and `--basic-user` / `--basic-pass` to the
> CLI. With both left empty the API is open on localhost.

## 4. Browse the message history

```bash
curl http://localhost:7600/v1/sessions/$SID/messages | jq
```

## 5. Use the web UI

The release binary already serves the web UI: open `http://localhost:7600/`
for the chat UI and `http://localhost:7600/admin/` for the admin console.

To run the chat UI from source against the same backend:

```bash
cd frontend
pnpm install
pnpm -F @xiaoguai/chat-ui dev
# → http://localhost:5173 (proxies /v1 to the backend)
```

## What ships in the compose stack

- **xiaoguai-core** — REST + SSE API on :7600, backed by embedded SQLite,
  serving the bundled chat UI (`/`) and admin UI (`/admin/`).
  - **Local-first by default**: the seeded `ollama-local` provider serves
    `qwen2.5-coder`, so the agent talks to a **local** model on boot — no
    cloud API key required. Start Ollama and pull the model:
    `ollama pull qwen2.5-coder` (or, with Docker, uncomment the `ollama`
    service in `deploy/docker-compose.yml` and
    `docker compose exec ollama ollama pull qwen2.5-coder`).
  - Point at a remote / GPU Ollama with the standard `OLLAMA_HOST` env var
    (e.g. `OLLAMA_HOST=http://10.0.0.5:11434`) — no config change needed.
  - Cloud providers (OpenAI / Anthropic / …) stay registered as fallbacks;
    add more with `xiaoguai provider register ...`. If the providers table
    is empty, core falls back to `MockBackend` so a bare stack still boots.

> An optional Valkey/Redis cache can be wired via `cache.url`; with none
> configured Xiaoguai uses an in-process cache, so no sidecar is needed.

**Air-gapped**: with local Ollama for chat (and a local embedding model),
the stack needs no outbound internet. Note: Ollama-backed embeddings for the
memory subsystem are a tracked follow-up — today the memory crate's only real
embedder is OpenAI-backed, so memory/recall in a fully air-gapped deployment
is pending that work.

## Next steps

- Wire your own MCP server: `xiaoguai mcp register --name fs
  --transport stdio --command npx --args
  '-y,@modelcontextprotocol/server-filesystem,/workspace'`.
- Register a real LLM provider: `xiaoguai provider register --name
  deepseek --kind openai_compat --endpoint https://api.deepseek.com/v1
  --api-key-env DEEPSEEK_API_KEY --models deepseek-chat`.
- Run it as a service: install the `.deb`/`.rpm` (or the tarball + the unit in
  `deploy/systemd/`) and manage it with `systemctl`.
- For Feishu integration, configure the webhook URL to
  `https://your-host/v1/im/feishu/webhook` and the encrypt key into
  `XIAOGUAI_IM_FEISHU__ENCRYPT_KEY`.

## Troubleshooting

| Symptom                               | Likely cause                                   |
|---------------------------------------|------------------------------------------------|
| `healthz` returns nothing             | Core not up — check `docker compose logs xiaoguai-core`. |
| `POST /v1/sessions` returns 500       | Migrations haven't run — `docker compose restart xiaoguai-core` reruns them against the SQLite file. |
| SSE stream stays empty                | MockBackend is configured to respond with a fixed string; for richer output configure a real provider. |
| 401 on `/v1/**`                       | An `auth.username`/`auth.password` is configured — pass HTTP Basic credentials (`curl -u user:pass`), or clear them for an open localhost run. |
