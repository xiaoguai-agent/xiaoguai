# Peer-pair example

A runnable two-process demo of the multi-agent peer topology described
in `docs/architecture/multi-agent-peer.md`. One xiaoguai-core instance
plays the **specialist** ("summarize_webpage"); a second plays the
**front-door** and delegates to the specialist via MCP.

```
┌────────────────────────┐         POST /v1/sessions/.../messages
│ user                   │ ──────────────────────────────────────┐
└────────────────────────┘                                       │
                                                                 ▼
                                              ┌──────────────────────────────┐
                                              │ front-door  (port 7612)      │
                                              │   ReactAgent + Toolbox       │
                                              │     summarize_webpage ──────┐│
                                              └──────────────────────────────┘
                                                                 │
                                                         MCP /v1/mcp/serve
                                                                 ▼
                                              ┌──────────────────────────────┐
                                              │ specialist  (port 7611)      │
                                              │   ReactAgent + Toolbox       │
                                              │     summarize_webpage (impl) │
                                              └──────────────────────────────┘
```

## Prereqs

- A built `xiaoguai-core` binary on `$PATH` (or use
  `cargo run -p xiaoguai-core --` from the repo root).
- Two empty databases (`xiaoguai_specialist`, `xiaoguai_frontdoor`)
  and Valkey running locally. The shipped `deploy/docker-compose.yml`
  brings these up.
- An LLM provider reachable from both peers (Ollama, OpenAI, etc.).
  Register it once in each peer via `xiaoguai provider register`.

## Five-step run

### 1. Start the specialist

```bash
XIAOGUAI_MCP__PUBLISH=true \
XIAOGUAI_AUDIT_SIGNING_KEY=dev-only-rotate-me \
xiaoguai-core --config examples/multi-agent/peer-pair/specialist/config.yaml serve
```

It binds `127.0.0.1:7611` and mounts `/v1/mcp/serve`.

### 2. Register the specialist's one tool

In a second terminal, point the specialist at whatever backend
implements `summarize_webpage`. The simplest choice for a demo is a
local stdio MCP server you control:

```bash
xiaoguai mcp register \
  --target http://127.0.0.1:7611 \
  --name local-summariser \
  --transport stdio \
  --command 'node ./tools/summariser-mcp.js'
```

(For a more realistic specialist, replace this with whatever capability
the specialist is meant to host: a RAG MCP server, the official GitHub
MCP server, an internal HTTP tool, etc.)

Verify the tool list:

```bash
curl -s http://127.0.0.1:7611/v1/admin/marketplace | jq '.entries[] | .tools'
```

### 3. Start the front-door

```bash
XIAOGUAI_AUDIT_SIGNING_KEY=dev-only-rotate-me \
xiaoguai-core --config examples/multi-agent/peer-pair/front-door/config.yaml serve
```

It binds `127.0.0.1:7612`. Note we deliberately do **not** set
`XIAOGUAI_MCP__PUBLISH` here — the front door is the topology's tip.

### 4. Wire the specialist into the front-door's Toolbox

```bash
xiaoguai mcp register \
  --target http://127.0.0.1:7612 \
  --name peer-specialist \
  --transport http \
  --endpoint http://127.0.0.1:7611/v1/mcp/serve
```

The `McpSupervisor` (v0.9.4.1 live-pickup wiring) connects on the next
reconcile, calls `list_tools` on the specialist's published endpoint,
and adds `summarize_webpage` to the front-door's shared Toolbox.

Confirm:

```bash
curl -s http://127.0.0.1:7612/v1/mcp/tools | jq '.tools[] | .name'
# expect to see "summarize_webpage" alongside the front-door's own tools
```

### 5. Drive a request that triggers the delegation

```bash
# Open a session and send a message that should route to the tool.
SID=$(curl -s -X POST http://127.0.0.1:7612/v1/sessions \
  -H 'content-type: application/json' \
  -d '{"user_id":"u1"}' | jq -r .id)

curl -N -X POST "http://127.0.0.1:7612/v1/sessions/${SID}/messages" \
  -H 'content-type: application/json' \
  -d '{"content":"Please summarise https://example.com/blog/something in 3 bullets."}'
```

You should see an SSE stream that includes a `tool_call` event for
`summarize_webpage`, then a `tool_result` event whose body originated
on the specialist, then the front-door's final assistant text built on
top of that result.

## Verifying the hop in audit logs

Each peer keeps its own audit chain (this is by design — see the
**Limitations** section of the architecture doc). After the run:

```bash
# Front door's chain: one tool_call to summarize_webpage.
curl -s 'http://127.0.0.1:7612/v1/admin/audit?action=tool_call' | jq

# Specialist's chain: one agent run triggered by the MCP call.
curl -s 'http://127.0.0.1:7611/v1/admin/audit?action=agent_run' | jq
```

Joining them today is a manual `timestamp + request_id` correlation;
a cross-peer audit stitch is on the v1.1.5b supervisor backlog.

## Tearing down

`Ctrl-C` on each terminal. Both peers honour graceful shutdown
(`tokio::signal::ctrl_c` in `xiaoguai-core/src/main.rs`).

## Further reading

- `docs/architecture/multi-agent-peer.md` — the pattern, why it works,
  when not to use it.
- `docs/plans/2026-05-23-v0.9.0.md` — `HttpMcpClient` (the MCP-client
  half that lets the front door reach the specialist).
- `docs/plans/2026-05-23-v0.9.1.md` — `/v1/mcp/serve` (the MCP-server
  half that lets the specialist be reached).
- `docs/plans/2026-05-24-v1.1.5b-supervisor.md` — the future
  centralised-orchestrator design (separate tag).
