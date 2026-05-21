# ADR-0003 — Diff-only file edits

Date: 2026-05-21
Status: Accepted

## Context

Research scan identified **whole-file rewrites as the largest cost-multiplier in agent tool design**:

- **Cline** users routinely report $50/day → $200/evening token bills. Root cause documented in user research: Cline **rewrites entire files for every edit** instead of emitting unified diffs.
- **Roo Code** explicitly forked Cline citing diff editing as the primary differentiator.
- **Cursor vs Cline** benchmarks: ~30% slower **and** higher cost on identical multi-file refactors.
- **LeanOps Q1-2026 agent cost audit**: full-file rewrites are the dominant contributor to >100× single-call multipliers at long task lengths.

Why this happens:
- Some LLMs (smaller local models especially) struggle with unified-diff output format.
- Naive tool design: `write_file(path, full_content)` is easier than `apply_diff(path, diff)`.
- Agent loops re-fetch + re-emit whole files because diff semantics are not enforced.

Cost impact compounds with iteration: a 5-iteration refactor on a 1000-line file = ~5,000 lines of LLM output × N round-trips. Diff version = ~50-100 lines per turn.

## Decision

Xiaoguai's file-mutation tooling is **diff-only by design**. There is **no `write_file(path, full_content)` MCP primitive in the core set**.

### Core tool surface

```
apply_diff(path, unified_diff)              -- only way to edit existing files
create_file(path, content, mode='new')       -- only when file does not exist
delete_file(path)                            -- explicit delete
move_file(from_path, to_path)                -- explicit rename
```

### Diff validation

Diffs go through `xiaoguai-mcp` validator before dispatch to MCP server:

1. Parse as unified diff format
2. Verify `from` lines match current file content (rejecting stale-base diffs)
3. **Reject** any single hunk that touches > 50% of file (heuristic anti-rewrite guard)
4. **Reject** total diff size > 30% of file unless `force_large_edit=true` is passed (must be set by user via UI confirmation, not by agent)

### MCP filesystem reference implementation

`mcp-server-fs-xiaoguai` (ship with v1.0) implements only the diff-only surface. We do **not** publish a `write_file` MCP server. Third-party MCP servers that expose `write_file` are flagged in admin-ui with "high cost risk" badge.

### Cost attribution to enforce the lesson

`budget_spend` (from ADR-0009) tags every spend with `(tool_name, file_path_hash, bytes_emitted)`. Admin-ui has a "whole-file rewrite suspect" panel listing edits where `bytes_emitted > 2 KB AND tool_name = write_file`. Operators can identify expensive third-party MCP servers and replace them.

### Adapter help for weak diff models

Some small local models cannot reliably emit unified diff. The **dialect adapter** (ADR-0005) layer offers a `pseudo_diff` mode where the model outputs a structured `find / replace` JSON pair, and the adapter converts to unified diff before dispatch. The adapter rejects ambiguous matches (multiple occurrences of `find` string) — forces model to provide more context.

## Consequences

**Positive:**
- Token cost 30-90% lower than Cline on identical refactors (matches Roo Code's measured improvement).
- Diff history naturally yields a clean git log when sessions are committed.
- "Whole-file rewrite suspect" panel makes economic outliers visible without log diving.
- Reduces hallucination surface: model can only change what it can describe in diff form.

**Negative:**
- Some legitimate operations (file format migration, generated-file regeneration) need whole-file replacement. Workaround: `delete_file` + `create_file` two-step, both audit-logged.
- Small local models with poor diff capability lose some tasks. `pseudo_diff` adapter mitigates but cannot eliminate.
- 50% / 30% thresholds are heuristic — may need per-tenant tuning.

**Mitigations:**
- `mcp-server-fs-xiaoguai` ships with built-in `find_unique(needle)` helper for the dialect adapter's ambiguity rejection.
- Workspace setting `large_edit_review_threshold` overrides the 30% global default.
- Eval harness (I5) includes a diff-correctness suite — regression-tests dialect adapter on each release.

## Implementation

- **v0.5.3**: `xiaoguai-mcp` validator rejects whole-file writes when diff-tool would suffice.
- **v0.5.2**: `xiaoguai-llm` dialect adapter `pseudo_diff` mode.
- **v0.5.4**: Agent loop prompt-builds tool schema with **only** `apply_diff` for existing files, `create_file` for new — agents physically cannot emit `write_file` for existing files.
- **v1.0**: `mcp-server-fs-xiaoguai` reference impl; admin-ui "rewrite suspect" panel.
- **v1.0**: eval harness diff-correctness suite (subset of SWE-bench Lite).

## References

- `docs/research/2026-05-21-local-agent-pain-points.md` §3.6
- qodo.ai Cline vs Cursor benchmark
- morphllm.com Roo Code vs Cline comparison
- LeanOps Q1-2026 agent cost audit
- ADR-0005 Local-LLM dialect adapter
- ADR-0009 Cost quota + token-bomb defense
