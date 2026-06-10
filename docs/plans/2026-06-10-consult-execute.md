# Implementation plan — T5: Consult/execute split + Agent Bridge

| | |
|---|---|
| Date | 2026-06-10 |
| Status | **APPROVED (owner blanket "全部执行" 2026-06-10)** — open questions flagged in the PR |
| Parent | `docs/plans/2026-06-09-capability-upgrade.md` §2-E / §3-T5 (after T4) |
| Hard constraints | DEC-033 unchanged |

## 0. Goal

An explicit, governed **mode** for agent turns: **consult = read-only** (the
agent can analyse but not mutate anything) vs **execute = normal HotL-gated
operation**. The "Agent Bridge" of the capability plan is a *semantic wrapper
over the existing HotL gate* — pure composition, no new gate infrastructure.

## 1. Grounding (verified 2026-06-10)

- No read/write classification survives into the toolbox today:
  `ToolDescriptor` (`crates/xiaoguai-mcp/src/types.rs:16`) has no metadata
  field; coding tools carry only textual `[READ]`/`[WRITE]` tags in
  descriptions (`crates/xiaoguai-coding/src/mcp_client.rs:56-178`); rmcp 1.7
  `ToolAnnotations.read_only` exists at the MCP layer
  (`crates/xiaoguai-mcp-exec/src/tools.rs:49`) but is **dropped on import**.
- Enforcement surfaces that exist: per-tool HotL gate in the ReAct loop
  (`hotl_gate.rs`, scope `tool_call.{name}`); per-turn toolbox construction in
  `turn.rs:153` (loop-tools precedent) and T4's `subset_toolbox`
  (`orchestrate.rs:205`).
- `SendMessageRequest` is `{content, model}` (`routes/sessions.rs:173`);
  sessions table has no metadata column.

## 2. Design — fail-closed, two enforcement layers

### 2.1 Tool mutation metadata

- `ToolDescriptor` gains `mutation_hint: MutationHint` (`Read | Write`),
  serde default **`Write`** — unannotated/unknown tools are consult-blocked
  (fail-closed).
- Populate: coding tools from their existing `[READ]`/`[WRITE]` tags (set the
  field explicitly, keep the tags); builtin loop tools = `Write`; **MCP import
  bridge**: when registering external MCP tools, map rmcp
  `annotations.read_only == true` → `Read`, else `Write` — external servers'
  self-declared read-only contract finally honoured.

### 2.2 Layer 1 — consult toolbox subset (visibility)

At turn launch (`turn.rs`), `mode == consult` → build a toolbox containing
only `mutation_hint == Read` tools (same mechanism as T4 `subset_toolbox`).
The model never sees write tools, so it doesn't attempt them.

### 2.3 Layer 2 — ConsultGate, the Agent Bridge (defense-in-depth)

`ConsultGate` wraps the session's `SharedHotlGate`: for `tool_call.{name}`
scopes where `{name}` is not in the resolved read-only set, return
`Deny("consult mode: write tools are disabled")` **without** consulting the
inner gate; everything else delegates. Lives next to the gate adapters in
`xiaoguai-core::hotl_bridge` (or `xiaoguai-agent` if no core types needed —
implementer's call, follow trait location). Denials surface to the model as
failed tool results (existing semantics) and are visible in events.

### 2.4 Mode plumbing + audit

- `SendMessageRequest.mode: Option<TurnMode>` (`consult | execute`, default
  execute) → `TurnInput.mode` → toolbox subset + gate wrap in `run_turn`.
- **No session schema change** (per-turn flag; UI makes it sticky). /loop and
  scheduler turns stay execute. **Orchestrate stays execute** — documented;
  consult-mode orchestration is a follow-up if a need appears.
- Audit: the `agent.run` audit entry gains `"mode"` in details; consult turns
  are distinguishable in the chain.

### 2.5 UI (chat-ui) — one control for T5 mode + T4 team runs

- Mode toggle by the send box: 执行/咨询 (execute default), sticky per
  session via localStorage; consult mode shows a subtle "read-only" cue on
  the input.
- **Team run entry (deferred from T4)**: when a team is attached, a "团队并行
  执行" action runs the current input through `orchestrateSession`, rendering
  member progress from the SSE events and the synthesized text as the reply
  bubble (orchestrate is always execute mode — disable the action in consult
  mode with a tooltip).
- shared client: `sendMessage` request body gains optional `mode`.

## 3. Tasks

| # | Task | Size | Verification |
|---|---|---|---|
| T5.1 | mutation_hint + populate + MCP import bridge + ConsultGate + turn plumbing + audit mode | M | unit: hint defaults/serde, bridge deny/delegate, subset; integration: consult turn cannot call write tool (both layers proven independently), execute unchanged, audit carries mode |
| T5.2 | shared client mode + chat-ui toggle + team-run entry | M | client tests; component tests: toggle sends mode, consult cue, team-run renders ExecEvents, disabled-in-consult |

## 4. Boundaries

- No session schema change; no consult-mode for loops/scheduler/orchestrate.
- No per-tool argument-level read/write analysis (a `Read` tool with
  exfiltration-ish args is HotL-policy territory, unchanged).
- DEC-033 intact; no new crates.

## 5. Open questions (defaults chosen, flagged in PR)

1. `execute_python` / `execute_javascript` sandbox tools: classified
   **Write** (they run arbitrary code; L1 sandbox does not block egress —
   audit round F3). Conservative default.
2. Coding `git_*` read tools (`git_status`/`git_diff`/`git_log`): `Read`.
   `exec`-style and mutation git tools: `Write`.
3. UI stickiness via localStorage (not server state) — acceptable for single
   owner; server-side session mode column deferred until asked for.
