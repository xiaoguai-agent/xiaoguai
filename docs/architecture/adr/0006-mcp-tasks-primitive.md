# ADR-0006 — MCP Tasks primitive (async, cancellable, status-pollable)

Date: 2026-05-21
Status: Accepted

## Context

The #1 user-trust killer across every researched agent platform is the **"Thinking..." indefinite hang** when a tool runs longer than the client's hardcoded timeout. Concrete incidents from `docs/research/2026-05-21-local-agent-pain-points.md`:

- **Cline #10853, #10631, #9700**: pending tool call hangs, requires window reload
- **claude-code #60866, #27431**: MCP tool calls hang when completed background agent exists
- **goose #9082, #4513**: stuck on non-blocking requests (`npm run dev` dev servers, long HTTP serves)
- **aider #4223**: stream parse error on long ollama chunk
- **AI Transfer Lab analysis**: "While the MCP hung, the agent's process held the model session, the gateway connection, and a worker thread."

Root cause: MCP standard tool calls are **synchronous request-response**. Every client wraps them in a hard timeout (10-30s typical) and assumes failure on expiry. Long-running tools — `terraform apply`, `vmware clone`, `npm run dev`, web scrape, large queries — have no protocol-level mechanism to say "this will take 20 minutes, here's my progress."

The **MCP Tasks primitive** (introduced in the November 2025 MCP spec) is the protocol-level fix: tools can declare async semantics, return a `task_id`, and clients poll for status. **No mainstream client has implemented this yet**.

## Decision

Xiaoguai implements MCP Tasks as a **first-class citizen** in `xiaoguai-mcp` from v0.5.3. Every tool call is treated as potentially-async; the supervisor + agent loop are built around the task model, not retrofitted onto a sync model.

### Tool declaration

MCP server manifests declare per-tool timing semantics:

```yaml
tools:
  - name: fs_read
    timing: sync         # default; hard deadline 30s
  - name: terraform_apply
    timing: async        # returns task_id; agent doesn't block
    estimated_duration_s: 600
    progress_supported: true
  - name: web_scrape
    timing: hybrid       # try sync first, escalate to async at deadline
    sync_deadline_s: 10
```

### Async task lifecycle

```
[agent emits tool_call]
        │
        ▼
[supervisor inspects timing]
        │
        ├── sync? → run with hard deadline → return result OR timeout error
        │
        └── async? → spawn worker, persist task row in PG
                        │
                        ├── status: queued → running → succeeded | failed | cancelled
                        ├── progress: optional `progress_pct` + `progress_msg`
                        ├── partial result: optional intermediate snapshots
                        └── final: full result, ts_completed
```

Database:

```sql
CREATE TABLE mcp_tasks (
    id              UUID PRIMARY KEY,
    tenant_id       TEXT NOT NULL,
    session_id      TEXT NOT NULL,
    tool_call_id    TEXT NOT NULL,    -- LLM-emitted reference
    mcp_server      TEXT NOT NULL,
    tool_name       TEXT NOT NULL,
    args_hash       BYTEA NOT NULL,
    status          TEXT NOT NULL,    -- queued | running | succeeded | failed | cancelled | timed_out
    progress_pct    SMALLINT,
    progress_msg    TEXT,
    started_at      TIMESTAMPTZ,
    completed_at    TIMESTAMPTZ,
    result_ref      UUID,             -- → mcp_result_blobs (ADR-0002 spill)
    error           TEXT,
    deadline_at     TIMESTAMPTZ NOT NULL,
    cancel_requested BOOL NOT NULL DEFAULT false
);
CREATE INDEX ix_mcp_tasks_status_deadline ON mcp_tasks (status, deadline_at) WHERE status IN ('queued', 'running');
```

### Agent loop integration

When agent emits an async-declared tool call:
1. Supervisor returns immediately with `{status: "started", task_id, estimated_duration_s}`
2. Agent receives this as the tool result; the assistant message includes `[task started: task_id, eta: N min]`
3. Agent can **continue with other tool calls** while task runs (parallel composition)
4. Agent can `mcp_task_status(task_id)` to poll
5. Agent can `mcp_task_cancel(task_id)` to abort
6. When task completes, supervisor writes result row + emits **server-sent event** to chat-ui / IM gateway:
   - chat-ui: card updates inline
   - 飞书/钉钉/企微: card patch via `chat.update` API (R3 in I9)

### User UX

- **chat-ui**: every in-flight async task shown as a card with progress bar + [Cancel] button. Stuck > 2× estimated duration → automatic alert with [Cancel] [Wait] [Background].
- **IM card**: same model, mobile-optimized — completion triggers push notification to phone (R3 mobile pattern).
- **CLI**: `xiaoguai task list/show/cancel` for ops-level intervention.

### Circuit breaker integration (cross-ADR)

Per-tool deadline + 3-strike circuit breaker (ADR-0009) applies to async tasks too:
- 3 consecutive `timed_out` outcomes for `(tenant, tool)` → circuit opens for 5min
- Cancel-rate dashboard surfaces tools users abort frequently → operator signal

### Failure recovery

Task rows are durable in PG. If `xiaoguai-core` restarts:
- Tasks with status `running` are reconciled at startup: probe child process via `mcp_task_status`; if process dead → mark `failed`
- Workers re-attached to live MCP processes via PID + supervisor handle
- Pending cancellations applied

## Consequences

**Positive:**
- Eliminates the #1 "Thinking..." hang failure class — agent **never** blocks indefinitely on a tool
- Long-running ops (VM clone, terraform, big batch) become natural patterns instead of agents-of-the-gaps hacks
- IM users get push notifications when tasks complete — phone-first UX works
- Combined with ADR-0008 trust receipts: every task lifecycle event is hmac-chained → auditable proof of completion
- Anticipates protocol-level acceptance — when other clients adopt, we're already compatible

**Negative:**
- Significant complexity vs sync-only MCP: needs persistence, state machine, polling, event delivery, restart reconciliation
- MCP server authors must declare timing semantics; existing community MCP servers may not (we treat unmarked as `sync`, may hit limits)
- Polling loop has latency floor — when agent needs result immediately, sync still wins on simple cases

**Mitigations:**
- Ship reference async-aware MCP servers (`mcp-server-terraform`, `mcp-server-web-scrape`) so community has templates
- Provide `xiaoguai-mcp-task-wrapper` that retrofits sync MCP server into async-capable wrapper for ops who can't modify upstream
- Default polling interval 2s with exponential backoff up to 30s — keeps latency reasonable

## Implementation

- **v0.5.3 Task 1c**: protocol implementation + PG schema + agent loop integration
- **v0.5.3 Task 11**: per-call deadline + 3-strike circuit breaker
- **v0.5.5**: server-sent event push to chat-ui + REST `GET /v1/tasks/:id` endpoint
- **v0.5.6**: 飞书 IM card patch + push notification on completion
- **v1.0**: CLI `xiaoguai task list/show/cancel`; cancel-rate operator dashboard

## References

- MCP November 2025 spec — Tasks primitive
- `docs/research/2026-05-21-local-agent-pain-points.md` §3.1
- Cline #10853 #10631 #9700, claude-code #60866 #27431, goose #9082 #4513
- AI Transfer Lab: "Why your MCP agent keeps timing out and the fix that just shipped"
- ADR-0008 Tool receipt provenance (task lifecycle hmac-chained)
- ADR-0009 Cost quota + circuit breaker
