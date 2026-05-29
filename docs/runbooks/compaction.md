# Session compaction — operator runbook

> Shipped in v0.5.4.1 — Tier-2 of the pi/Hermes roadmap. The companion
> design philosophy is in
> [`xiaoguai-agent-design/docs/harness-engineering.md`](https://github.com/xiaoguai-agent/xiaoguai-agent-design/blob/main/docs/harness-engineering.md)
> §4 (Token transformation pipeline).

## What this is

Long agent sessions — especially against local Ollama models with a
32k-token context — used to fail with provider 400s when the
conversation grew past the model's window. The old behaviour
([`history::slide`](../../crates/xiaoguai-agent/src/history.rs)) dropped
the oldest messages wholesale, which lost facts the agent later needed
to answer follow-up questions.

`history::compact` replaces the head of the conversation with an
LLM-generated summary while keeping the most recent N turns verbatim
and the system prompt intact. On summariser failure (network blip,
empty output, timeout) it falls back to the old slide behaviour.

## When it kicks in

Compaction is **off by default**. To enable it, configure the agent:

```rust
let cfg = xiaoguai_agent::AgentConfig::new("qwen2.5-coder")
    .with_compaction(xiaoguai_agent::history::CompactionConfig {
        max_context_tokens: 30_000,
        trigger_at_pct: 75,
        keep_recent: 6,
        summary_model: "qwen2.5-coder",
    });
```

Or in `~/.xiaoguai/local.yaml`:

```yaml
agent:
  compaction:
    enabled: true
    max_context_tokens: 30000
    trigger_at_pct: 75
    keep_recent: 6
    summary_model: qwen2.5-coder
```

Once enabled the loop, **before each LLM request**, estimates the
current conversation's token count and triggers compaction if it
exceeds `trigger_at_pct%` of `max_context_tokens`.

## Tuning by model

| Model | `max_context_tokens` | `trigger_at_pct` | Notes |
|---|---:|---:|---|
| `qwen2.5-coder` (32k) | 30_000 | 75 | Default. 2k headroom for tools + next turn. |
| `llama3.1:8b-instruct` (128k) | 100_000 | 80 | Larger window; can wait longer to compact. |
| `gpt-4o-mini` (128k) | 100_000 | 80 | Same. Provider-side caching may already help — measure first. |
| `claude-haiku-4-5` (200k) | 150_000 | 85 | Prefer Anthropic `cache_control` when possible; compaction is a fallback for non-cache-friendly conversations. |
| `gemini-1.5-pro` (1M) | 900_000 | 90 | Effectively never compact in practice. |

`keep_recent: 6` (≈ 3 user/assistant exchanges) is a sane default. Bump
to 10–12 if your conversations have many short tool-call cycles —
keeping more raw context costs more tokens but preserves more
verifiable detail.

## What the summary contains

The summariser receives a `system` message instructing it to:

- Produce a dense plain-text summary in ≤ 500 tokens.
- Keep **concrete facts** (names, IDs, file paths, error codes,
  decisions).
- Drop pleasantries and commentary.
- **Do NOT invent facts** not present in the input.

The output replaces the older head messages with a single synthetic
`Role::System` message tagged
`"[Compacted summary of N earlier messages]\n\n..."`. The agent reads
it like any other system context.

## Metrics

Three Prometheus series are emitted:

| Metric | Labels | Meaning |
|---|---|---|
| `xiaoguai_compaction_triggered_total` | `reason=threshold\|manual` | Every compaction attempt |
| `xiaoguai_compaction_fallback_total` | `reason=backend_error\|empty_summary` | Attempts that fell back to slide |
| `xiaoguai_compaction_token_savings` | (histogram) | Tokens dropped per successful compaction |

Healthy ratio in production: `fallback / triggered` < 5 %. If higher,
the summariser model is unreliable — try a different `summary_model`
or disable compaction.

## Debugging "agent forgot something it should know"

Symptom: the agent answers correctly for ~50 turns, then suddenly
forgets a fact from earlier.

Triage:

1. **Compact happened mid-fact?** Check
   `xiaoguai_compaction_triggered_total`. If 0 since session start,
   compaction isn't the cause.
2. **Summary too lossy?** Pull the synthetic summary message from the
   stored session (`SELECT content FROM messages WHERE session_id = ?
   AND role = 'system' AND content LIKE '[Compacted summary%' ORDER BY
   created_at DESC LIMIT 1`). If the missing fact is absent there, the
   summariser dropped it.
3. **Recover the fact**: store it as a long-term memory via
   `xiaoguai memory add` (see [`local-memory-and-redaction.md`](local-memory-and-redaction.md)).
   Long-term memory is retrieved by similarity and bypasses compaction.
4. **Adjust `keep_recent`** to retain more raw context if your
   conversations are bursty (lots of fact-establishment at the start
   followed by long execution tails).

## What this is NOT

- It does NOT replace [`xiaoguai-memory`](../../crates/xiaoguai-memory/).
  Long-term facts belong in memory; compaction is a short-term context
  bound.
- It does NOT do hierarchical summary-of-summaries. After 10
  compactions the synthetic summaries accumulate. If you see drift,
  manually trigger a fresh session and import the last summary as the
  new system prompt.
- It is NOT enabled by default on Anthropic/OpenAI backends — those
  providers have native prompt caching that's strictly better. Enable
  compaction only when your traffic pattern doesn't fit caching.

## Related

- Design rationale:
  [`harness-engineering.md`](https://github.com/xiaoguai-agent/xiaoguai-agent-design/blob/main/docs/harness-engineering.md)
  §4 (R.E.S.T → Efficiency; token transformation pipeline)
- Memory runbook:
  [`local-memory-and-redaction.md`](local-memory-and-redaction.md)
- HotL gate (a sibling lever for resource control):
  [`hotl-escalation-stuck.md`](hotl-escalation-stuck.md)
- Plan:
  [`../plans/2026-05-28-tier2-next.md`](../plans/2026-05-28-tier2-next.md)
  §D.2
