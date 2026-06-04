# Multi-agent peer topology

> **Status:** v1.1.5a shipped. This doc describes the peer pattern.
> A second tag (v1.1.5b) ships the supervisor design doc separately
> at `docs/plans/2026-05-24-v1.1.5b-supervisor.md`.

## TL;DR

Every running `xiaoguai-core` instance is *already* both:

- An **MCP server**: when `XIAOGUAI_MCP__PUBLISH=true`, the v0.9.1 route
  `/v1/mcp/serve` exposes its `Toolbox` as a Streamable-HTTP MCP server.
  External agents (or *other xiaoguai instances*) connect and call any
  tool the local agent loop can call.
- An **MCP client**: since v0.9.0, `HttpMcpClient` can connect to any
  Streamable-HTTP MCP server and surface its tools into the local
  `Toolbox`. The marketplace + supervisor (v0.9.4 / v0.9.4.1) consume
  remote MCP servers this way.

Compose those two facts and you have a peer-to-peer multi-agent topology
**without writing a new crate.** A "specialist" agent is a xiaoguai-core
instance whose Toolbox is intentionally narrow (one tool: `summarize`,
or `review_pr`, etc.) and whose system prompt is tuned for that one
job. A "front-door" agent is another xiaoguai-core instance whose
Toolbox includes — via `HttpMcpClient` pointed at the specialist's
`/v1/mcp/serve` — the specialist's tool. When the model on the front
door decides to call `summarize`, the call is dispatched through MCP to
the specialist, which runs *its own* ReAct loop to satisfy the call,
and returns the result back through MCP.

There is no new orchestration crate. The MCP `call_tool` handler on the
specialist side already spawns a remote ReactAgent run via the existing
v0.9.1 server handler. The peer pattern is a *deployment shape*, not a
code shape.

## Why this works

The key invariant from v0.9.1 (`crates/xiaoguai-api/src/mcp_serve.rs`):
> "whatever tools an internal agent sees, an external agent sees verbatim."

When the front door calls `summarize` on the specialist's published
endpoint, the specialist's `XiaoguaiMcpServer::call_tool`:

1. Looks up `summarize` in its own `Toolbox`.
2. Dispatches to the registered `Arc<dyn McpClient>` for that tool.

For a *leaf* tool (e.g. the specialist's `Toolbox` registers a stdio
backend that hosts an `Ollama` or a local RAG client), that's it — the
backend runs and returns. But the specialist is free to wrap *its own
ReactAgent* behind a tool: a custom `McpClient` impl whose `call_tool`
constructs a `RuntimeContext`, runs `xiaoguai_runtime::run_to_completion`,
and returns the final assistant text. From the front door's
perspective, it called one tool; the specialist secretly ran a whole
agent loop to answer.

This is the same trick Anthropic's "subagents" pattern (system prompt
tells the model "use the `dispatch_to_subagent` tool when …") plays at
the model layer — except here we move it down a layer to MCP so the
front door doesn't need to know it's talking to another agent.

## When to use peer

The peer shape is good for:

- **Capability specialisation.** A "code-review" specialist with a
  narrow Toolbox (read file, run lints, list git diff) + a tuned
  prompt outperforms a generalist with 80 tools. The front door asks
  the specialist by name.
- **Trust isolation.** Each peer is a separate process (its own owner, its
  own auth gate, its own audit chain) and publishes only the tools it chooses
  over MCP. The front door cannot ever see tools the specialist didn't publish.
- **Independent scaling.** The summarize specialist can run on a box
  with one big GPU; the front door can run cheap CPU instances. They
  speak HTTP.
- **Polyglot specialists.** Anything that speaks MCP works — the
  specialist doesn't even need to be xiaoguai. A peer can be Continue,
  the GitHub MCP server, or a python `fastmcp` worker.

## When *not* to use peer

The peer shape is **not** good for:

- **Shared session state across hops.** Today every tool call is a
  fresh agent run on the specialist side. There is no shared
  `RuntimeContext`, no shared session id, no continuity. If the
  specialist needs the front door's chat history to answer, the front
  door must pass it explicitly as a tool argument. This is the same
  limitation the v0.9.1 deferred list calls out: "today there's no
  shared session across MCP hops."
- **Multi-step plans that need to back-and-forth across peers.** If
  the front door wants the specialist to do step 1, then summarise
  step 1, then decide step 2 *based on the front door's wider
  context*, the round-trips add up. The supervisor pattern
  (v1.1.5b plan doc) is a better fit.
- **Cancel propagation.** Cancelling the front-door run does not today
  cancel an in-flight specialist run. The specialist will finish its
  call and return. A real supervisor needs cancel-token plumbing
  across the wire — also v1.1.5b territory.
- **Audit chain consolidation.** Each peer appends to *its own* audit
  chain. The front door's chain records "called tool X on remote";
  the specialist's chain records "ran agent loop for X". There is no
  cross-peer chain stitch — operators have to join on timestamps +
  request ids if they want one timeline.

## Limitations summarised

| Concern              | Today                                                                 | Future work    |
|----------------------|----------------------------------------------------------------------|----------------|
| Session state        | Every `call_tool` is a fresh agent run on the specialist side.       | v1.1.5b        |
| Cancel propagation   | Cancelling front door does not cancel in-flight specialist.          | v1.1.5b        |
| Audit chain stitch   | Each peer has its own chain; cross-peer correlation is by timestamp. | v1.1.5b        |
| Auth on peer hops    | Anonymous if `mcp_publish_enabled = true`. Use `HttpClientConfig::with_auth` to pass a static token. | v0.9.1 deferred |
| Tool list refresh    | `list_changed: false`; front door does not learn about new specialist tools without reconnect. | v0.9.1 deferred |
| Loop detection       | Nothing prevents specialist A from publishing a tool that calls back to front door, etc. | v1.1.5b should add a hop counter |

## The example

`examples/multi-agent/peer-pair/` ships a runnable two-process demo:

- `specialist/config.yaml` — single-purpose "summarize webpage" agent.
- `front-door/config.yaml` — the user-facing agent. Its `Toolbox`
  consumes the specialist via `HttpMcpClient`.
- `README.md` — five-step run instructions.

The example is intentionally minimal: it exercises one tool hop. The
front door's system prompt says "if the user asks to summarise a URL,
call the `summarize_webpage` tool." The specialist's system prompt
says "you are a summariser; produce a 3-bullet summary." Real
deployments will obviously want more.

## The integration test

`crates/xiaoguai-core/tests/peer_mvp.rs` (added in v1.1.5a) spawns
both peers in-process on `127.0.0.1:0` ports via
`serve_with_state_and_extras`, then:

1. Connects the front-door's `Toolbox` to the specialist's
   `/v1/mcp/serve` via `HttpMcpClient`.
2. Asserts the specialist's tool is visible in the front-door
   `Toolbox::to_specs()`.
3. Drives an end-to-end POST to the front door that triggers a tool
   call into the specialist; asserts the response includes the
   specialist's output.

The test runs without Docker; the only network is two `127.0.0.1`
TCP listeners on ephemeral ports. PG / Valkey are not exercised
because the in-memory repos from `xiaoguai-api/tests/common/` are
reused. See the test source for the exact wiring.
