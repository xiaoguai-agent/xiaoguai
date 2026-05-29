# Session handoff — Sprint-9 complete, ready for Sprint-10

> Written 2026-05-31. The user is clearing the conversation; the next
> session starts from this doc.

---

## TL;DR (one paragraph)

Sprint-9 (planner/worker/critic triangle, DEC-021) shipped end-to-end:
8 PRs merged (6 implementation + 2 design), 153 tests passing in
`xiaoguai-orchestrator`, **v1.7.0 released**. The workflow rule
documented in `~/.claude/projects/.../memory/sprint-workflow.md` —
七步循环 (1. 架构文档 → 2. 安排任务 → 3. 审核 → 4. 执行 → 5. Merge
→ 6. 推 git → 7. 发 release) — was followed end-to-end and works.
**Next session must start with Sprint-10 Step 1**: architecture docs
in the design repo first (DEC-022 details + new `lld-observability.md`
SLO section).

---

## What shipped (Sprint-9, all merged)

| PR | Branch (deleted) | Phase | Content |
|---|---|---|---|
| **#82** | `feat/sprint9-s9-1-triangle-scaffolding` | A | triangle/ scaffolding (types + traits + 27 tests) |
| **#87** | `feat/sprint9-s9-2-planner-agent` | B-A | PlannerAgent (1 LLM call + Plan JSON parse + retry) |
| **#86** | `feat/sprint9-s9-3-worker-agent` | B-B | WorkerAgent (wraps ReactAgent max_iterations=1 for budget gating) |
| **#88** | `feat/sprint9-s9-4-critic-agent` | C | CriticAgent (1 LLM call → Verdict) |
| **#89** | `feat/sprint9-s9-7-s9-8-tags-and-runbook` | E (parallel) | migration 0025_persona_role_tags.sql + triangle-pattern runbook |
| **#91** | `feat/sprint9-s9-5-s9-6-triangle-pattern` | D | Triangle pattern wiring + 6 integration tests |

Design repo: **PR #4** (lld-orchestrator §4.4–4.7 + lld-personas v1.6+ note + PHILO §13 triangle row).

**v1.7.0 release**: <https://github.com/xiaoguai-agent/xiaoguai/releases/tag/v1.7.0>

## What's queued — Sprint-10

Architecture is already committed at the **HLD level** (DEC-022 in
sprint-8 PR design-repo#1 — "4 SRE golden signals as first-class SLO
contracts with burn-rate alerts"). What sprint-10 needs:

### Sprint-10 Step 1 — architecture detail PR (design repo)

Author **next**, before any task plan or code. Mirror what sprint-9 did
in design-repo PR #4 — drop one layer deeper than the top-level DEC.

Files to update/create:
1. **NEW `xiaoguai-agent-design/docs/lld/lld-observability.md`** — full LLD for the SLO module:
   - SLO struct shape (signal, threshold, window, burn-rate-fast/slow)
   - 4 golden-signal definitions in xiaoguai context
   - Default SLO values (already approved by user; published in sprint plan PR #77 §10)
   - `xiaoguai_slo_burn_rate{signal,window}` Prometheus metric design
   - Integration with existing `xiaoguai-watch` for alert routing (no new alertmanager plumbing)
   - Per-tenant SLO override via `tenant_settings.settings->>'slo_*'` JSONB (mirror sandbox_tier pattern from sprint-8 DEC-019)
2. **UPDATE `xiaoguai-agent-design/docs/harness-engineering.md` §16** — metrics → embed SLO/burn-rate framing on top of the 4-bucket taxonomy
3. **UPDATE `xiaoguai-agent-design/docs/RELEASE-LOG.md`** — new row for sprint-10 Step 1

Branch: `feat/sprint10-slo-lld-detail` (in the design repo, base on main).

### Sprint-10 Step 2 — task plan (implementation repo)

After Step 1 merges, write `docs/plans/2026-06-01-sprint-10-slo.md`
following the same 8-section + 6-point self-review template.

Tasks (from `docs/plans/2026-05-30-sprint-8-10-roadmap.md` §4 sprint-10):
- S10-1: `docs/runbooks/slo.md` (table + page chain per signal) — 0.5d
- S10-2: burn-rate alerts in `deploy/helm/xiaoguai-observability/alerts/` (fast 1h + slow 6h × 4 signals) — 1d
- S10-3: `xiaoguai_slo_burn_rate{signal,window}` Prometheus metric in `xiaoguai-observability` — 1d
- S10-4: Grafana dashboard `slo-overview.json` with 4-signal panels + budget tracker — 1d
- S10-5: wire alerts into `xiaoguai-watch` so escalation goes through existing channels — 1d
- S10-6: failure-mode runbook entries — one per signal × {fast-burn, slow-burn} = 8 entries — 1.5d
- S10-7: update `harness-engineering.md` §16 (this also goes into Step 1 PR; keep here as a track-completion entry) — 0.5d

**Sprint-10 total**: ~ 6.5 dev-days (shortest sprint). Documentation-heavy.

### Sprint-10 sub-agent dispatch plan

| Phase | Sub-agents | Tasks |
|---|---|---|
| A (I drive) | none | S10-3 metric instrumentation (small, single file) |
| B (1 sub-agent) | 1 | S10-2 alerts + S10-4 Grafana dashboard (single sub-agent, both files) |
| C (I drive) | none | S10-1 runbook + S10-5 watch wiring + S10-6 8 runbook entries + S10-7 PHILO update |

Disk peak: 1 worktree × ~ 30 GB. Well within budget.

### Sprint-10 reviewer-asks already approved

User signed off on these in sprint-8 sign-off (PR #77 §9):
- SLO defaults from DEC-022 — approved as starting points (no
  pre-calibration round on session-7 production data needed)
- 3-sprint split (sprint-10 is the third)

No new asks needed from the user before dispatching — the design PR
can go straight up.

---

## What's still open / outstanding

### Open follow-up tracks (no specific sprint slot yet)

| Track | What | Where |
|---|---|---|
| **T1** sprint-7 | Operator-driven live demo recording for `xiaoguai-mcp-exec` | needs operator with running PG + Ollama; script in `docs/scripts/demo-mcp-exec.sh` |
| **L3 sandbox follow-ups** | refresh-token at-rest encryption hardening (T4 follow-up), PDF rendering for non-trivial templates, T3 production wiring polish | various |
| **Triangle promotion side effect** | sprint-9 deferred memory write-back from Approved scratchpad → session memory (option b). Follow-up PR needed | `xiaoguai-orchestrator::patterns::triangle.rs` |
| **`MultiLevelTriangle`** | Planner spawning sub-Planners. Reserved; not on any roadmap |  |
| **macOS notarisation + Windows EV signing** | Tier-3 polish; deferred pending paid certs ($99/yr Apple + ~$300/yr EV) |  |

### Open PRs in main repo at handoff time

| PR | Title | Status |
|---|---|---|
| #58 | test: cover backup crypto/integrity paths in xiaoguai-cli | Existing (not session-9) — operator review when convenient |
| #62/#63 | Dependabot (cargo-minor-patch, age) | Existing; merge when convenient |

(Sprint-9 PRs all merged + branches deleted.)

---

## Workflow rule reminder

`~/.claude/projects/.../memory/sprint-workflow.md` is **load-bearing** —
the user told us "以后都是这个规则" (this rule applies to every sprint
from now on). Seven steps:

```
1. 更新架构文档    ← design repo PR first; one layer deeper than top-level DEC
2. 安排任务        ← implementation repo plan PR after step 1 merges
3. 审核           ← user sign-off; do NOT execute before this
4. 执行           ← sub-agents for parallel work; main worktree for serial
5. Merge          ← order matters; rebase + force-push if base branch deleted
6. 推 git         ← both repos
7. 发 release     ← gh release create vX.Y.Z + handoff
```

Sprint-8 = v1.6.0. Sprint-9 = v1.7.0. Sprint-10 will be v1.8.0.

### Lesson from sprint-9: PR auto-close after base-branch deletion

When PR `feat/sprint9-s9-2-planner-agent` had its base
`feat/sprint9-s9-1-triangle-scaffolding` squash-merged with
`--delete-branch`, GitHub auto-closed the PR. Cannot be reopened. Fix:
rebase locally on `origin/main`, force-push, **open a fresh PR**
(#85 → #87, then later #90 → #91).

When dispatching sub-agents based on an unmerged-PR branch, anticipate
this: either merge the PR first (serial) or accept the recreate-PR
cost (parallel). Sprint-9 chose parallel; cost was small (~ 5 min per
recreated PR).

---

## How to resume

Open this directory in a fresh Claude Code session:

```bash
cd /Users/zw/testany/myskills/xiaoguai
claude
```

Memory will auto-load:
- `project-status.md` — current sprint state (updated 2026-05-31)
- `agent-roadmap.md` — Tier-1/2/3 progress (updated for sprint-9 completion)
- `ci-gotchas.md` — disk budget + worktree pitfalls
- `sprint-workflow.md` — the seven-step rule

Then say to the next session:

> "继续 sprint-10 Step 1 — 写 DEC-022 实施细节到设计仓 lld-observability.md"

The next session should:
1. Read `docs/HANDOFF-2026-05-31-sprint9-complete.md` (this file)
2. Read `docs/plans/2026-05-30-sprint-8-10-roadmap.md` §4 sprint-10 row
3. Read `xiaoguai-agent-design/docs/hld.md` DEC-022 (sprint-8 PR #1)
4. Start writing the new `lld-observability.md` in the design repo on a fresh branch

If the next session asks "what should I do next?", point at the
"Sprint-10 Step 1" section above.

---

## File pointers for next session

- This handoff: `docs/HANDOFF-2026-05-31-sprint9-complete.md`
- Sprint-9 retro plan: `docs/plans/2026-05-31-sprint-9-multi-agent.md`
- Sprint 8–10 roadmap: `docs/plans/2026-05-30-sprint-8-10-roadmap.md`
- Workflow rule: `~/.claude/projects/.../memory/sprint-workflow.md`
- Memory index: `~/.claude/projects/.../memory/MEMORY.md`
- Design docs: <https://github.com/xiaoguai-agent/xiaoguai-agent-design>
- Releases: <https://github.com/xiaoguai-agent/xiaoguai/releases>

---

## TL;DR (one more time)

✅ Sprint-9 done. v1.7.0 out.
⏭ Next: Sprint-10 Step 1 = lld-observability.md (SLO module LLD) in
**design repo, on a new branch**. Do not touch implementation code
until Step 1 merges, Step 2 (task plan PR) is written, and step 3
(user review) gives a green light.
