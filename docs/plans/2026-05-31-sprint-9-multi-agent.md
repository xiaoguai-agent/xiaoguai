# Sprint-9 — Multi-agent planner/worker/critic triangle

> Companion to `xiaoguai-agent-design#4` (DEC-021 + `lld-orchestrator.md`
> §4.4–4.7 + new `triangle/` submodule layout). Per workflow rule
> (`sprint-workflow.md`): **this is step 2 (任务安排); step 3 (审核)
> gates step 4 (执行)**.

---

## 1. Context

Sprint-8 (v1.6.0) closed Tier-2/3 functional items. Sprint-9 implements
DEC-021's planner/worker/critic triangle. Architecture detail landed in
the design-repo PR — this plan operationalises it.

The original 2026-05-30 roadmap allotted 10 dev-days; the architecture
detail PR moved the LLD work (formerly S9-7) to Step 1, so this sprint
shrinks to ~ 9 dev-days. Still fits one week.

R.E.S.T: Reliability primary (Critic catches Worker errors before they
propagate); §5 PHILO Decision↔Execution separation made literal.

---

## 2. Sprint-9 backlog table

| Pri | ID | Task | Depends on | Est. | R.E.S.T axis |
|:-:|---|---|---|---:|---|
| P0 | **S9-1** | `triangle/` submodule scaffolding: `Plan`, `Task`, `TaskId`, `Role`, `Verdict`, `Scratchpad`, `MemoryView`, `MemorySnapshot`, `TriangleBudget` types + traits. Compiles; no behaviour. | merge of design PR #4 | 1 day | scaffolding |
| P0 | **S9-2** | `PlannerAgent`: thin wrapper around `ReactAgent` that parses LLM output as `Plan` JSON (acceptance criteria + retry on malformed) | S9-1 | 2 days | R |
| P0 | **S9-3** | `WorkerAgent`: reuses `ReactAgent`; writes intermediate state to `Scratchpad`; final `WorkerResult` includes artefact + citations + confidence + cost | S9-1 | 2 days | R |
| P0 | **S9-4** | `CriticAgent::review(result, criteria) -> Verdict`: one LLM call; verdict enum is `Approve(reason)` / `RequestRevision(feedback)` / `Reject(reason)` | S9-1 | 1 day | R |
| P1 | **S9-5** | `BudgetEnforcer` split 50/40/10 + `Triangle` pattern impl in `patterns/triangle.rs` wiring §4.4 algorithm + emitting `OrchEvent` variants | S9-2, S9-3, S9-4 | 1 day | E |
| P1 | **S9-6** | Integration tests — the 6 cases enumerated in lld-orchestrator §7 (triangle_happy_path, request_revision, reject_triggers_replan, scratchpad_quarantine, budget_split_enforced, replan_cap_terminates) | S9-5 | 1.5 days | R + T |
| P1 | **S9-7** | Migration `0025_persona_role_tags.sql` — adds optional `tags TEXT[]` column to `personas` + index | none | 0.5 day | T |
| P2 | **S9-8** | Runbook `docs/runbooks/triangle-pattern.md` — when to use, persona naming convention, budget tuning, failure-mode triage | S9-5 | 0.5 day | T |

**Sprint-9 total**: ~ 9 dev-days. Sub-agent dispatch limited because
S9-2/3/4 share the trait definitions from S9-1 — serial early, parallel
late.

---

## 3. Sub-agent dispatch plan

| Phase | Sub-agents | Range |
|---|---|---|
| Phase A | (none — I drive) | S9-1 scaffolding alone, in main worktree. Locking down trait shapes early avoids 3-way contention. |
| Phase B | **2 parallel** sub-agents | A: S9-2 (PlannerAgent). B: S9-3 (WorkerAgent). Both depend only on S9-1's trait surface; they touch disjoint files. |
| Phase C | (I drive) | S9-4 (CriticAgent) — small + single-LLM-call surface; finished in ~ 4 hours of focused work, not worth a worktree. |
| Phase D | **1 sub-agent** | S9-5 (`Triangle` pattern wiring) + S9-6 (6 integration tests). Single sub-agent because S9-6 directly verifies S9-5. |
| Phase E | I drive in parallel with Phase D | S9-7 (migration) + S9-8 (runbook). |

Disk budget: max 3 concurrent worktrees × ~ 30 GB = ~ 90 GB peak (well
within the ~ 200 GB free we're tracking).

---

## 4. Cross-sprint risks (Sprint-9-specific)

| Risk | Mitigation |
|---|---|
| `PlannerAgent` produces non-JSON output (LLM ignored the structured-output instruction) | One-shot retry with the error injected as context; on second failure emit `Final { stop_reason: PlannerFailed }`. Tested in S9-6's malformed-plan case (added there even though §7 doesn't list it — judgement). |
| Critic gets stuck in a request-revision loop | Hard cap `max_revisions` per task (default 3); after cap, force `Reject(too_many_revisions)` and hand back to Planner. Codified in §4.4 algorithm + S9-6 test. |
| Cross-Worker contamination via shared memory | `Scratchpad` is `task_id`-keyed and Workers never see other scratchpads. Test `triangle_scratchpad_quarantine` (S9-6) spawns two Workers concurrently and asserts neither can read the other's notes. |
| `BudgetEnforcer` rounding errors at low budgets | At budgets < 1000 tokens the 50/40/10 rounding can produce per-role caps of 0 (worker_pct=50 * parent=10 / 100 = 5; then 50*1000/100 = 500 floors to 0 at parent=1). Validate `parent_budget >= 100` at pattern dispatch; fail with `BudgetTooSmall` early. |
| `MemorySnapshot::captured_at` skew across roles | Snapshot taken once at the start of each plan→execute round and passed by reference to all three roles; never re-captured mid-round. Codified as DEC-021 §4.5 invariant. |
| `Verdict::RequestRevision(feedback)` confused with `Reject` | Lib-level naming + serde-tagged enum; S9-4 unit tests assert round-trip + display. |

---

## 5. Out of scope (Sprint-9)

- Hierarchical planning (planner-of-planners). Reserved for sprint-11+
  if we ever need it.
- Critic-as-deterministic-checker (no LLM call). The brief considered
  this; the user explicitly approved LLM Critic in sprint-8 sign-off.
  Revisit only after production data shows cost regression.
- Persona-recommendation UI (admin-ui can add a "role suggester" later).
- Auto-budget-tuning per-persona based on outcome telemetry.

---

## 6. Workflow checkpoint

```
1. 更新架构文档    ✅ xiaoguai-agent-design#4
2. 安排任务        ✅ THIS PR
3. 审核           ← awaiting your sign-off
4. 执行           ← only after step 3
5-7. Merge/push/release ← after sprint ships → v1.7.0
```

After sprint-9's step 7 I automatically enter **sprint-10 Step 1** —
SLO + 4 golden signals + burn-rate alerts per DEC-022.

---

## 7. Self-review (6-point protocol)

| # | Check | Result |
|---|---|---|
| 1 | Cited file paths exist | **PASS** — lld-orchestrator.md §4.4–4.7 land in design PR #4; `xiaoguai-orchestrator/` is in the workspace today |
| 2 | Every task proposes runnable verification | **PARTIAL** — table-level; per-task `VC:` lines appear in sub-agent prompts |
| 3 | Each task has a measurable outcome | **PASS** — every row has est., dependency, R.E.S.T axis |
| 4 | Out-of-scope is honored | **PASS** — §5 lists 4 explicit non-goals (hierarchical planning, deterministic Critic, UI, auto-tuning) |
| 5 | Risks have mitigations | **PASS** — §4 has 6 specific risks each with concrete mitigation |
| 6 | Time estimate sane | **PASS** — ~ 9 dev-days (1 + 2 + 2 + 1 + 1 + 1.5 + 0.5 + 0.5 = 9.5), fits one week with sub-agent parallelism |

**Soft spots flagged for reviewer**:

1. **Phase B parallel dispatch** — Worker and Planner sub-agents both need
   the trait surface from S9-1. If sprint-9's S9-1 lands a trait that's
   wrong, two sub-agents waste work simultaneously. Mitigation: I review
   S9-1 carefully before dispatching B; if anything looks off, I do S9-2
   serially and only parallelise once Plan parsing is solid.
2. **Phase D single sub-agent doing both S9-5 and S9-6** — large scope. If
   the sub-agent gets blocked on S9-5, S9-6 starves. Alternative: dispatch
   S9-5 to sub-agent, do S9-6 myself after it returns. Trade-off: serial
   adds 1-2 days wall clock vs slightly safer dispatch.

---

## 8. Asks for the reviewer (you)

Before I drive Phase A:

1. **Phase B parallelism approved?** Lock S9-1 trait shapes first, then
   dispatch S9-2 + S9-3 in parallel. Recommended over fully serial.
2. **S9-7 migration column type** — `tags TEXT[]` (Postgres native array)
   vs `tags JSONB`. JSONB matches the pattern from sprint-8 (we picked
   JSONB for `tenant_settings`); TEXT[] gives a clean `WHERE 'role/critic'
   = ANY(tags)` query. Recommendation: TEXT[] because queries against
   role tags are pure equality and the indexing story is simpler.
3. **S9-8 runbook scope** — should it include a worked example (full
   chat transcript through the triangle for a "research and summarise"
   goal)? Adds ~ 0.5 day. Recommendation: yes, runbooks without examples
   tend to rot.
4. **Critic max_revisions default** — 3 sub-revisions per task before
   forcing Reject. OK or want a different number?
