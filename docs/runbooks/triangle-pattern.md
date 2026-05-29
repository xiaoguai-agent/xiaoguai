# Triangle pattern operator runbook

> Sprint-9 (v1.6+, DEC-021). Companion to
> `xiaoguai-agent-design/docs/lld/lld-orchestrator.md` §4.4–4.7.

## What this pattern is

The **planner-worker-critic triangle** is a structured multi-agent topology where three roles cooperate on one goal:

| Role | Job | Default budget share |
|---|---|---|
| **Planner** | Decompose the goal into a `Plan` of `Task`s | 40 % |
| **Worker** | Execute one `Task` as a full ReAct loop | 50 % |
| **Critic** | Review each `WorkerResult`; Approve / RequestRevision / Reject | 10 % |

Critic's small budget is by design — it makes accept/reject decisions, not artefacts. If you find the Critic budget regularly exhausted, the right fix is usually to **shrink the rubric** in `AcceptanceCriteria`, not to bump the budget.

## When to use this pattern

| Use this when … | Use a simpler pattern when … |
|---|---|
| Work is **heterogeneous** (research → synthesise → quality-check) | Work is **homogeneous parallel** (run the same analysis 5x and aggregate — use v1.4 `RoundRobin` instead) |
| Each task has a **clear pass/fail rubric** | The output is fuzzy / preference-based (a single ReAct agent does fine) |
| **Auditability matters** — a Critic verdict per task is your audit trail | The task is interactive chat |
| You can afford **3 LLM personas + cost per task** | Latency budget < 5 s round-trip |

## Setting up personas with role tags

After applying migration `0025_persona_role_tags.sql`, tag your personas so the orchestrator and the admin-ui can find them:

```sql
UPDATE personas
SET tags = ARRAY['role/planner', 'domain/k8s']
WHERE name = 'k8s-incident-planner';

UPDATE personas
SET tags = ARRAY['role/worker', 'domain/k8s']
WHERE name = 'k8s-investigator';

UPDATE personas
SET tags = ARRAY['role/critic', 'domain/k8s']
WHERE name = 'k8s-strict-reviewer';
```

Tags are a **discovery convenience only** — the orchestrator doesn't enforce that a `role/critic`-tagged persona must be used as a Critic. A persona tagged `role/critic` can still be invoked as a Worker if you point the Triangle config at it. Role distinction lives at the pattern layer, not the persona layer.

Query patterns:

```sql
-- List all candidate Critics
SELECT id, name FROM personas WHERE 'role/critic' = ANY(tags);

-- Find personas that fit two roles (intersection)
SELECT id, name FROM personas WHERE tags && ARRAY['role/critic', 'role/worker'];
```

## Worked example: research-and-synthesise

Goal: *"Summarise the SOC2 audit findings from the last 30 days and recommend three remediation actions."*

```yaml
# orchestrator config snippet
pattern:
  kind: triangle
  planner: role/planner-default
  worker: role/worker-research
  critic: role/critic-strict
  budget_split:
    worker_pct: 50    # default
    planner_pct: 40
    critic_pct: 10
  max_replans: 3
  max_revisions_per_task: 3
```

Round 1 — Planner emits:

```json
{
  "round": 1,
  "goal": "Summarise SOC2 audit findings ... and recommend 3 actions",
  "tasks": [
    {"description": "List audit_log entries with action='audit.finding' from last 30 days; group by severity",
     "acceptance_criteria": {
       "rubric": "answer enumerates each severity bucket with a count",
       "required_citation_pattern": "audit_log",
       "min_confidence": 0.7
     }},
    {"description": "For each top-3 finding, draft a remediation action and rank by impact",
     "acceptance_criteria": {
       "rubric": "exactly 3 actions, each with rationale and estimated impact",
       "min_confidence": 0.6
     }}
  ]
}
```

Task 1 — Worker queries the audit log, scratchpads its intermediate steps, returns a structured summary. Critic checks: does the answer enumerate each severity? Yes → **Approve**. Scratchpad migrates into round-2 memory.

Task 2 — Worker drafts 3 actions. Critic checks: exactly 3? Each ranked? First draft has 4 actions → **RequestRevision** with `feedback: "the rubric calls for exactly 3 actions; pick the top 3"`. Worker re-runs once, returns 3 → **Approve**.

Final summary aggregates the two artefacts. Total LLM calls: 1 (Planner) + ~ 4 (Worker task 1) + 1 (Critic) + ~ 2 (Worker task 2) + 2 (Critic, twice) = ~ 10 calls. Budget was 50/40/10 of, say, 10 000 tokens → 5000 Worker / 4000 Planner / 1000 Critic. Critic consumed ~ 600 tokens across 3 verdicts; comfortable.

## Tuning the budget split

The default (50/40/10) suits most workflows. Bump the Critic share when:

- Auditors require detailed verdicts (more reasoning per Critic call)
- The rubric requires citation pattern matching against long artefacts
- Replans are common — extra Critic budget catches reject-able tasks earlier

Bump the Planner share when:

- Goals are open-ended (more re-planning rounds expected)
- Plans typically need to be regenerated mid-execution

Reduce the Critic share to 5 % when:

- The acceptance criteria are short and deterministic
- The Critic persona is a small / fast model

## Failure-mode triage

| Symptom | Likely cause | Fix |
|---|---|---|
| `Final { stop_reason: PlannerFailed("malformed JSON after 2 attempts") }` | Planner model can't produce valid JSON | Use a stronger Planner persona (e.g. claude-sonnet-4-6); add explicit JSON-schema example to the persona system prompt |
| `Final { stop_reason: MaxReplansReached }` | Every plan gets rejected — Critic and Planner disagree on what success looks like | Read the Critic Reject reasons in the final summary; rewrite the rubric to be measurable |
| `BudgetExhausted { role: Critic }` | Rubrics too long / artefacts too long | Shrink rubric OR increase Critic share (see "Tuning the budget split") |
| `BudgetExhausted { role: Worker }` | Worker hit MaxIterations or ran ReAct too long | Shrink the task scope in the Plan; bump `max_iterations` per Worker; or move to a multi-task Plan with smaller tasks |
| Every task gets `RequestRevision` 3× and is forced to Reject | Rubric requires something Worker can't produce (e.g., citation pattern that doesn't exist in the data) | Re-examine `required_citation_pattern` — is the data labelled the way the pattern expects? |
| Plan validation always rejects with "duplicate ids" | Bug — TaskIds are generated by orchestrator, not Planner; Planner shouldn't emit ids | Check the persona system prompt — does it include the example "do NOT include task ids"? |

## What this pattern is NOT

- **NOT a workflow engine**. There's no DAG runtime, no on-disk state machine. The triangle is a structured *conversation* dispatcher — if your problem is "run these 5 steps in this exact order with retries and notifications", that's a job for the scheduler + skill packs, not the triangle.
- **NOT hierarchical**. The Planner can't spawn a sub-Planner. Reserved for a future `MultiLevelTriangle` if we ever need it (not on any roadmap today).
- **NOT a code reviewer**. A persona tagged `role/critic` reviews artefacts against rubrics — it does not statically analyse code, run tests, or compare diffs. Use the code-review skill pack for that.
- **NOT a guarantee**. The Critic uses the same LLM family as the Planner and Worker; it's a sanity check, not a formal verifier. For high-assurance workflows wire a HotL escalation under the orchestrator (see `runbooks/hotl-escalation-stuck.md`).

## Related

- LLD: [`lld-orchestrator.md`](https://github.com/xiaoguai-agent/xiaoguai-agent-design/blob/main/docs/lld/lld-orchestrator.md) §4.4–4.7
- Persona conventions: [`lld-personas.md`](https://github.com/xiaoguai-agent/xiaoguai-agent-design/blob/main/docs/lld/lld-personas.md) v1.6+ note
- DEC-021 architecture: HLD §3
- Sprint plan: [`docs/plans/2026-05-31-sprint-9-multi-agent.md`](../plans/2026-05-31-sprint-9-multi-agent.md)
