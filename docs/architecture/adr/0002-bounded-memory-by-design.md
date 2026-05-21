# ADR-0002 — Bounded memory by design

Date: 2026-05-21
Status: Accepted

## Context

Research scan of 7 competing agent projects (see `docs/research/2026-05-21-local-agent-pain-points.md`) identified **OOM in long sessions** as one of the top-2 user-trust killers:

- **Cline #8868**: heap OOM at only ~10MB of task history
- **qwen-code #4167**: V8 Mark-Compact during compression OOMs (2GB old-space ceiling)
- **Cline #6696, #7959, #8208**: related memory exhaustion issues
- **goose #9153**: 3M-char MCP response leaked Docker container memory

Root cause is shared: Node.js / V8 2GB old-space ceiling collides with unbounded data structures (chat history `Vec<Message>`, tool result blobs, MCP response buffers, sub-agent context inheritance).

Rust does **not** automatically solve this. A Rust agent with an unbounded `Vec<Message>` will eventually OOM the host, just on a different timeline. The structural fix is **explicit caps and spill-to-disk**, not memory safety.

## Decision

Xiaoguai enforces **hard memory bounds at every accumulation point**, configurable per tenant:

| Resource | Default cap | Spill strategy |
|---|---|---|
| Per-session message history | 200k tokens | LRU evict oldest non-pinned; if pinned exceeds cap, LLM-summary compaction with user approval |
| Single MCP tool response | 1 MB | Spill body to PG `mcp_result_blobs` table; return `{ref: <id>, preview: <2KB>}` to LLM |
| Total MCP responses cached in-memory | 100 MB per process | LRU evict to disk |
| Tool call result cache | 50 MB per session | LRU evict |
| Sub-agent depth | 3 levels | Refuse spawn at depth 4; agent must continue in current context |
| Parallel tool invocations | 5 concurrent | Queue beyond cap; emit "waiting" status |
| Agent loop iterations | 50 per turn | Forced abort with `no_progress_detected` error |
| Audit log retention in-memory | 0 — always disk | Direct write to PG, no in-memory queue |

A `cargo bench` regression test simulates a 1000-turn session and **fails the build** if peak RSS exceeds 1.5× of the configured cap envelope.

## Consequences

**Positive:**
- Rust + bounded design beats Node/V8 not by language alone but by **explicit engineering discipline**. Marketing line "no OOM in 24h sessions" becomes provable.
- Predictable resource cost per tenant — enables accurate sizing and per-tenant cgroup limits.
- Spill-to-disk indirection layer (`McpResultRef`) becomes natural place to add provenance hashing (ADR-0008).

**Negative:**
- Adds latency on cache-miss for spilled blobs (PG read).
- Compaction-with-user-approval is more friction than silent compaction — but explicit > silent is the correct default (matches Goose #9330 backlash).
- Sub-agent depth 3 is restrictive; complex tasks may need to be re-modeled as sequential rather than recursive.

**Mitigations:**
- Cache hot blobs in Valkey (default TTL 5min) to reduce PG hits.
- Provide `xiaoguai-cli session pin <message_id>` for users who legitimately need long-context.
- Document the depth limit + provide "task graph" alternative for complex workflows.

## Implementation

- `xiaoguai-types`: introduce `BoundedHistory<T>` and `BoundedCache<K, V>` types with explicit caps in constructor.
- `xiaoguai-storage`: `mcp_result_blobs(id PK, session_id FK, content_hash, body)` table.
- `xiaoguai-mcp`: response interceptor — if body > cap, spill + return ref.
- `xiaoguai-agent`: iteration counter, sub-agent depth tracker, parallel-tool semaphore.
- `xiaoguai-config`: all caps exposed as tenant settings with sensible defaults.
- Memory bench in CI (`cargo bench --bench memory_envelope`).

## References

- `docs/research/2026-05-21-local-agent-pain-points.md` §3.2
- Cline #8868, qwen-code #4167, goose #9153, goose #9330
