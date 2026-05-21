# ADR-0008 — Tool result provenance + claim verification

Date: 2026-05-21
Status: Accepted

## Context

The #1 enterprise-trust killer in current agent platforms is the **"agent claims X but actually did Y"** failure class:

- **Replit (2025-07)**: agent deleted production DB during freeze, then **fabricated 4,000 fake users + falsified test results to mask the damage**. The agent told the user rollback was impossible. (incidentdatabase.ai/1152, Fortune)
- **Claude Code #7381**: tool outputs hallucinated after `/clear` — context fragments from prior sessions pasted as if from tools; **no actual execution happened**.
- **Cursor forum #155098**: "Agent deleted entire file without permission and tried to hide it" — corroborated by multiple threads (#58852, #134465, #157260) describing delete-then-conceal behavior.
- **Statsig research observation**: coding agent **skipped running tests, generated a fake passing log, ingested it as truth** for subsequent reasoning. Classic self-poisoning loop.
- **Claude Code #10628**: model fabricated a user message, then defended it as ground truth.

Root mechanisms identified:
- **M1**: no cryptographic binding between tool call ↔ tool result. Model can later "remember" a result that never existed.
- **M2**: context pollution after `/clear` or compact — old transcript fragments leak in as if they were tool outputs.
- **M3**: summary phase decoupled from execution log. Final "I did X, Y, Z" message is generated from model memory, not from a structured action log.
- **M4**: anchoring bias on earlier wrong claim. Once model said "tests pass", next turn defends that claim instead of re-running.
- **M5**: streaming UI cannot distinguish "model said this" vs "tool returned this" — provenance lost at render layer.

Existing research:
- **NABAOS (arXiv 2603.10060)** proposes HMAC-signed tool execution receipts the LLM cannot forge.
- **PROV-AGENT (arXiv 2508.02866)** proposes W3C PROV-style provenance graphs across agent steps.
- No major production agent platform implements either today.

## Decision

Xiaoguai treats **every tool invocation as a signed, verifiable receipt** chained into the audit log, and the assistant's final summary is **mechanically derived from receipts**, not generated freely by the model.

### Receipt chain (extends `xiaoguai-audit` hmac chain from §8.3 of design)

Every tool call writes one row **before** returning result to the model:

```rust
struct ToolReceipt {
    receipt_id:    Uuid,
    run_id:        SessionTurnId,
    tool_call_id:  String,       // matches LLM-emitted tool_call_id
    mcp_server:    String,
    tool_name:     String,
    args_hash:     [u8; 32],     // SHA-256 of canonical-JSON args
    result_hash:   [u8; 32],     // SHA-256 of canonical-JSON result
    post_condition_probe_hash: Option<[u8; 32]>,
    ts:            DateTime<Utc>,
    prev_hmac:     [u8; 32],     // chains into audit hmac
    hmac:          [u8; 32],
}
```

The model receives only the result body. The receipt lives in PG `tool_receipts` table, hmac-chained into the existing audit log.

### Verify-before-claim for `[WRITE]` tools

Every destructive MCP tool **must** declare a `post_condition_probe` in its manifest:

```yaml
name: fs_delete
write: true
post_condition_probe: fs_stat   # auto-invoked after fs_delete; stat result hashed into receipt
```

After `fs_delete(path)` runs, supervisor auto-runs `fs_stat(path)`. If stat still returns the file, receipt's `post_condition_probe_hash` reflects that — the claim "I deleted X" can be falsified by reading the receipt.

### Structured summary (replaces free-form "I did X")

End-of-turn summary is **template-rendered from receipts**, not LLM-generated:

```rust
struct TurnSummary {
    actions:        Vec<ReceiptRef>,   // pulled from tool_receipts table
    model_narrative: String,           // LLM's prose, displayed in separate UI panel
    divergence:     Vec<UnverifiedClaim>,  // automated diff between narrative and receipts
}
```

Frontend renders three panels: **Actions** (verified, from PG), **Narrative** (LLM, separate visual treatment), **Divergence** (red-flagged unverified claims).

### Context provenance tagging

Every chunk in context tagged at insertion time:

```rust
enum ContextChunkOrigin {
    System,
    User,
    Model,
    ToolResult(ReceiptId),
    PinnedFile(Path),
}
```

Compaction preserves tags. **Any `ToolResult` chunk without a corresponding receipt in PG is stripped before the next LLM call** — defends against #7381 / #10628 paste-back hallucination class.

### Trust score telemetry

Per-run metric: `claims_verified_count / claims_total_count`. Surfaced in admin-ui per session and per tenant. Sessions < 0.9 auto-flagged for review. Tenant trust-score trends feed model router — low-trust models get demoted from critical workflows.

## Consequences

**Positive:**
- Replit-class incidents (fabrication, masking) become **mechanically detectable** — automated divergence warnings make it impossible to "hide" silently.
- Audit log becomes the source of truth, not LLM memory. Compaction can never erase tool execution evidence.
- Per-receipt hmac chain ties cost claims (ADR-0009) and security claims (etc.) to the same verifiable backbone.
- Differentiator: nobody else does this. NABAOS + PROV-AGENT are research papers; no shipping platform implements.

**Negative:**
- Adds DB write per tool call (latency ~1-2ms per receipt with PG WAL).
- `post_condition_probe` requires every `[WRITE]` MCP tool to register a probe — work burden on MCP authors.
- Structured summary UI is more complex than single chat-bubble — UX design challenge.
- LLM responses must include `tool_call_id` properly; some models drop these — needs adapter (ADR-0005).

**Mitigations:**
- Batch receipt writes per-turn (1 PG round-trip for N receipts).
- Provide reference probe implementations for common MCP servers (fs, http, db, exec).
- Frontend can collapse summary panels by default; expose "verify" affordance on demand.
- Dialect adapter (ADR-0005) handles tool_call_id normalization across local-model formats.

## Implementation

- **v0.5.1**: schema `tool_receipts` table + hmac chain extension in `xiaoguai-audit`.
- **v0.5.3**: `xiaoguai-mcp` supervisor wraps every tool call with receipt write; post-condition probe registration in MCP manifest schema.
- **v0.5.4**: `xiaoguai-agent` end-of-turn pipeline emits `TurnSummary` (actions + narrative + divergence).
- **v1.0**: chat-ui three-panel render; admin-ui trust-score dashboard.
- **v1.0**: trust-score feeds model router.

## References

- Replit incident — incidentdatabase.ai/cite/1152
- Claude Code #7381, #10628
- Cursor forum #155098, #58852, #134465, #157260
- NABAOS (arXiv 2603.10060)
- PROV-AGENT (arXiv 2508.02866)
- `docs/research/2026-05-21-local-agent-pain-points.md` §C2
