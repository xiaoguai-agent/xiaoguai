# MiniMax provider runbook

> Sprint-8 S8-10 / DEC-024. The MiniMax backend lives in
> `crates/xiaoguai-llm/src/minimax.rs`; the registry row is seeded by
> migration `0023_minimax_provider_seed.sql`.

## What this provider gives you

- An OpenAI-compatible LLM backend pointed at
  `https://api.minimax.io/v1/chat/completions`.
- **Thinking-mode passthrough**: M1/M2 family models stream a
  `reasoning_content` field on each SSE chunk; we expose those bytes via
  `ChatChunk.reasoning_delta` (new optional field added in S8-10) so the
  agent loop can display, log, or feed it to a Critic without mixing
  reasoning into the assistant `content` channel.
- A Prometheus counter
  `xiaoguai_llm_reasoning_tokens_total{provider="minimax", model=...}`
  recording the estimated reasoning-token throughput.

## Models seeded

| Model | Thinking mode | Notes |
|---|:---:|---|
| `MiniMax-M1` | yes | First-gen reasoning model |
| `MiniMax-M2` | yes | Current production reasoning model |
| `MiniMax-M2.5` | yes | Mid-cycle update |
| `MiniMax-M2.7` | yes | Latest M2 patch line |
| `abab6.5-chat` | no | Chat-only; `reasoning_delta` stays `None` |

## Enabling the provider

The migration leaves MiniMax **disabled** — the row exists with
`default_for_models='[]'::jsonb`, so the router never auto-picks it. To
enable for tenants:

```sql
UPDATE llm_providers
SET default_for_models = '["MiniMax-M2","MiniMax-M2.5"]'::jsonb,
    updated_at = NOW()
WHERE id = 'minimax-system';
```

Restart `xiaoguai serve` so the router rebuilds.

## API key

Set `MINIMAX_API_KEY` in the runtime environment. The provider row
references this env var via `api_key_env='MINIMAX_API_KEY'`. Sources:

- MiniMax open platform: <https://platform.minimaxi.com/document>

## Thinking-mode cost note

MiniMax bills reasoning tokens at the **output-token rate**. A chain-of-
thought that runs 5 000 tokens before a 200-token answer costs you the
sum.

If you observe cost regressions after enabling M1/M2:

1. Compare `xiaoguai_llm_reasoning_tokens_total` vs
   `xiaoguai_token_usage_total` (output) per tenant — a high ratio
   means reasoning dominates.
2. Consider routing low-stakes prompts to `abab6.5-chat` (no reasoning)
   and reserving M2 for high-value calls.
3. Cap per-tenant via the existing budget rails — the budget enforcer
   counts both content and reasoning tokens.

## Observability quickref

```promql
# Reasoning throughput per model (tokens/sec, last 5m)
rate(xiaoguai_llm_reasoning_tokens_total{provider="minimax"}[5m])

# Reasoning-to-output ratio (a stand-in for "how much CoT cost")
sum(rate(xiaoguai_llm_reasoning_tokens_total{provider="minimax"}[15m]))
 /
sum(rate(xiaoguai_token_usage_total{provider="minimax",direction="output"}[15m]))
```

## Reasoning bytes in the agent loop

Today (S8-10) `ChatChunk.reasoning_delta` is captured and metered but
the ReAct loop does **not** persist it to the assistant message — that
wiring is a Sprint-9 follow-up (DEC-021 Critic feeds on reasoning bytes).

## Troubleshooting

**Symptom**: HTTP 401 from MiniMax.
- Check `MINIMAX_API_KEY` is loaded into the process.

**Symptom**: Empty `reasoning_delta` on M2 calls.
- Confirm the model name matches the seeded model strings byte-for-byte
  (`MiniMax-M2`, not `minimax-m2`).

**Symptom**: `decode SSE` errors in logs.
- Likely transient API change. File a bug with captured payload.

## Out of scope

- UI affordance for the `reasoning_delta` channel (Sprint-9, DEC-021).
- Per-call thinking-mode toggle (multi-provider abstraction).
- Backfilling reasoning bytes into the audit chain.
