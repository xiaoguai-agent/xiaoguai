# Implementation plan — T7: Memory — team glossary + import/export

| | |
|---|---|
| Date | 2026-06-10 |
| Status | **APPROVED (owner blanket "全部执行" 2026-06-10)** — open questions flagged in the PR |
| Parent | `docs/plans/2026-06-09-capability-upgrade.md` §2-H / §3-T7 |
| Hard constraints | DEC-033 unchanged |

## 0. Goal & honest scope

Capability plan §2-H says "team glossary + unify local/IM/knowledge-base
memory sources; optional import". The explore pass (2026-06-10) found:

- Memory (semantic store), IM history, and RAG are **deliberately separate
  stores with different semantics**; "unification" would be a read-time
  `CompositeMemoryView` — but the production MemoryView bridge the
  orchestrator expects (`triangle/memory_view.rs:7`) **doesn't exist yet**,
  and stuffing IM+RAG+memory into every turn risks context bloat.
- The high-value, low-risk pieces are: **team glossary** (USER.md injection
  precedent, `identity.rs` + `turn.rs:212`) and **memory import/export**
  (no surface exists at all).

**T7 therefore ships: glossary + import/export + source tags + admin-ui
wiring. The CompositeMemoryView unification is explicitly deferred** until
the orchestrator memory bridge exists and an eval can guard context bloat
(flagged §5).

## 1. Design

### 1.1 Team glossary (migration 0034)

- `ALTER TABLE expert_teams ADD COLUMN glossary_md TEXT` — optional markdown
  (terminology, constraints, procedures), capped at 16 KiB on write
  (identity.rs's 8 KiB cap precedent ×2; boundary-validated, 400 over cap).
- `Team.glossary_md: Option<String>` through model/repos/routes
  (Create/Update requests; PATCH semantics like other fields).
- **Injection** (mirrors USER.md): in `run_turn`, when the session has a team
  attached and that team carries a glossary, insert a system message
  `"Team glossary (<team name>):\n<md>"` AFTER the identity message. Same
  injection for T4 orchestrate member runs (each member sees the team
  glossary). Not persisted into history (identity precedent).
- Three-tier context model documented: USER.md (owner identity) → team
  glossary (team knowledge) → persona system_prompt (role).

### 1.2 Memory source tags (convention, no schema)

Standard tag prefix `source:` (`source:imported`, `source:im`, `source:rag`)
documented + applied by the import path. Pure convention over the existing
`tags` column; recall/list filtering by tag already works.

### 1.3 Import / export

- `GET /v1/memories/export?kind=` → JSONL (one `{kind, content, tags,
  ttl_at, created_at}` per line; embeddings NOT exported — they're
  re-computed on import).
- `POST /v1/memories/import` → body = JSONL (text/plain), each line
  validated then created via the existing `MemoryStore::create_memory`
  (re-embeds); auto-tag `source:imported` if no `source:` tag present;
  response `{imported, skipped: [{line, reason}]}` — malformed lines are
  skipped and reported, not fatal (fail-soft for bulk files).
- CLI: `xiaoguai memory export [--kind] [--out FILE]` and
  `xiaoguai memory import FILE` — thin wrappers (direct store access via the
  local DB like provider commands; consistent with the CLI's local-first
  posture).
- Audit: `memory.import` (count) / `memory.export` (count) best-effort.

### 1.4 Frontend

- shared client: team glossary in Team types (already flows if added to the
  Rust DTO — verify serde), `exportMemories`, `importMemories`.
- admin-ui: ExpertTeams drawer gains a glossary textarea (cap hint);
  Memory pane gains Import (file → preview count → confirm) and Export
  buttons. (The pane's 404-fallback-era types in `frontend/shared/src/
  memory.ts` get reconciled with the real routes while touching this —
  verify what the pane uses today and fix only what this task touches.)

## 2. Tasks

| # | Task | Size | Verification |
|---|---|---|---|
| T7.1 | glossary: migration 0034 + model/repos/routes + turn & orchestrate injection | M | repo tests; route tests (cap 400); injection integration test (system message present, after identity; member runs too) |
| T7.2 | import/export routes + CLI + source-tag convention + audit | M | route tests (round-trip, fail-soft skips, auto-tag); CLI smoke via lib fn tests |
| T7.3 | shared client + admin-ui (glossary textarea, memory import/export) | S–M | client tests; pane component tests |

## 3. Boundaries

- **No CompositeMemoryView / IM / RAG unification** (deferred with rationale §0).
- No embedding export; no Claude-MEMORY.md format parsing (JSONL only, v1).
- DEC-033 intact.

## 4. Open questions (defaults chosen, flagged in PR)

1. Glossary also injected for **consult** turns — yes (it's read-only context).
2. Import dedup: none in v1 (re-import duplicates create twins) — flagged;
   content-hash dedup is a cheap follow-up if it bites.
3. IM→memory and RAG→memory promotion: future work, needs the memory bridge.
