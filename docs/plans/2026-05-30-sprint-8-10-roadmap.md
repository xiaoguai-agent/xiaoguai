# Sprint 8–10 roadmap — L3 sandbox + multi-agent + SLO + Tier-2/3 hardening

> Companion to `xiaoguai-agent-design#1` (DEC-019..023, the architectural
> commitments). Per workflow rule
> (`~/.claude/projects/.../memory/sprint-workflow.md`): architecture docs
> ship before this plan. **This is step 2 (任务安排); step 3 (审核) gates
> step 4 (执行).**

---

## 1. Context

Sprint-7 finished all functional items of Tier-1/2/3 (v1.5.0 released).
The user picked "全部" of the four candidate themes — so we plan all four,
but split across 3 sprints to keep each sprint shippable.

| Theme | DEC | Sprint |
|---|---|---|
| L3 sandbox implementation (wasmtime + pyodide + QuickJS-WASM) | DEC-019, DEC-020 | **8** |
| Tier-2/3 follow-up hardening (AES-GCM tokens, PDF, T3 PgRepo, T6 wiring) | DEC-023 | **8** (parallel) |
| Multi-agent planner/worker/critic triangle | DEC-021 | **9** |
| SLO + 4 golden signals + burn-rate alerts | DEC-022 | **10** |

Each sprint should land in ~ 1 week of focused work. Sprint-8 is heaviest
because L3 trait extraction is a prereq and follow-up hardening is 4
sub-tasks.

---

## 2. Sprint-8 backlog table (next week)

| Pri | ID | Task | Depends on | Est. | R.E.S.T axis |
|:-:|---|---|---|---:|---|
| P0 | **S8-1** | `ExecBackend` trait extraction in `xiaoguai-mcp-exec` + `xiaoguai-mcp-exec-js` (DEC-019) | merge of DEC-019..024 PR | 1 day | reliability scaffolding |
| P0 | **S8-2** | New crate `xiaoguai-mcp-exec-wasm` with `WasmtimePythonBackend` (DEC-020 — Python L3 first) | S8-1 | 3 days | S |
| P1 | **S8-3** | `WasmtimeJavaScriptBackend` (DEC-020 — JS L3 second) | S8-2 | 2 days | S |
| P1 | **S8-4** | Per-tenant `sandbox_tier` config + selector in `ExecServer::new` (DEC-019 wiring) | S8-1 | 1 day | E + S |
| P0 | **S8-5** | T4 AES-GCM refresh-token at rest (DEC-023.1) | none | 1 day | S |
| P1 | **S8-6** | T5 typst PDF rendering backend (DEC-023.2) | none | 1 day | T |
| P1 | **S8-7** | T3 production wiring: `PgSkillProposalRepository`, `PgTenantSettings`, `xiaoguai-core::skill_author_bridge` (DEC-023.3) | none | 1 day | S |
| P2 | **S8-8** | T6 agent-loop integration test for `execute_javascript` (DEC-023.4) | none | 0.5 day | T |
| P2 | **S8-9** | Update `lld/lld-mcp-exec.md` + `lld/lld-mcp-exec-js.md` for trait extraction; new `lld/lld-mcp-exec-wasm.md` | S8-2, S8-3 | 0.5 day | T |
| P0 | **S8-10** | **`MinimaxBackend` (DEC-024)** — new `crates/xiaoguai-llm/src/minimax.rs`, `0023_minimax_provider_seed.sql` migration, `ChatChunk.reasoning_delta` field, `xiaoguai_llm_reasoning_tokens_total` metric, runbook entry. Mirror `GroqBackend` pattern. | none | 1 day | E (provider routing) + T (reasoning visibility) |

**Sprint-8 total**: ~ 11 dev-days (was 10; +1 for S8-10). With 2 parallel sub-agents (Wasmtime path + hardening path including S8-10) wall-clock is ~ 5-6 days.

---

## 3. Sprint-9 backlog table (week after)

| Pri | ID | Task | Depends on | Est. | R.E.S.T axis |
|:-:|---|---|---|---:|---|
| P0 | **S9-1** | `Role` enum + `WorkerResult` + `MemoryView` trait in `xiaoguai-orchestrator` (DEC-021) | none | 1 day | R |
| P0 | **S9-2** | `PlannerAgent::plan(goal, context) -> Plan` | S9-1 | 2 days | R |
| P0 | **S9-3** | `WorkerAgent` re-uses existing `ReactAgent` + writes to private `Scratchpad` | S9-1 | 2 days | R |
| P0 | **S9-4** | `CriticAgent::review(result, criteria) -> Verdict` | S9-1 | 1 day | R |
| P1 | **S9-5** | Budget split 50/40/10 enforcement at supervisor level | S9-2, S9-3, S9-4 | 1 day | E |
| P1 | **S9-6** | Integration test: 5-step planner-worker-critic chain ends in approved artefact; Critic rejection triggers Worker re-plan | S9-2..5 | 2 days | R + T |
| P2 | **S9-7** | Update `lld/lld-orchestrator.md` with the triangle topology | S9-1..6 | 0.5 day | T |
| P2 | **S9-8** | Update `lld/lld-personas.md` — roles are persona presets | S9-1 | 0.5 day | T |

**Sprint-9 total**: ~ 10 dev-days. Sub-agent dispatch limited (more sequential than sprint-8).

---

## 4. Sprint-10 backlog table

| Pri | ID | Task | Depends on | Est. | R.E.S.T axis |
|:-:|---|---|---|---:|---|
| P0 | **S10-1** | Write `docs/runbooks/slo.md` (DEC-022 deliverable: SLO table + page chain per signal) | none | 0.5 day | T |
| P0 | **S10-2** | Define burn-rate alerts in `deploy/helm/xiaoguai-observability/alerts/` (fast 1h + slow 6h per signal) | none | 1 day | T |
| P0 | **S10-3** | Add `xiaoguai_slo_burn_rate{signal,window}` Prometheus metric in `xiaoguai-observability` | none | 1 day | T |
| P1 | **S10-4** | Grafana dashboard `slo-overview.json` with 4-signal panels + budget tracker | S10-3 | 1 day | T |
| P1 | **S10-5** | Wire alerts into `xiaoguai-watch` so escalation goes through existing channels (no new alertmanager) | S10-2, S10-3 | 1 day | T |
| P1 | **S10-6** | Failure-mode runbook entries: one per SLO signal × { fast-burn, slow-burn } = 8 runbook entries | S10-1 | 1.5 days | T |
| P2 | **S10-7** | Update `harness-engineering.md` §16 metrics — embed SLO/burn-rate framing | S10-1..5 | 0.5 day | T |

**Sprint-10 total**: ~ 6.5 dev-days. Shortest sprint; documentation-heavy.

---

## 5. Cross-sprint risks

| Risk | Mitigation | Sprint |
|---|---|---|
| wasmtime + pyodide cold start exceeds 10 ms target | Engine precompile cache at boot; benchmark in S8-2 with hard fail at > 50 ms | 8 |
| pyodide stdlib gaps (`subprocess`, `socket`) break existing agent snippets | Document explicit error messages; runbook §"What L3 doesn't support" | 8 |
| Planner-Worker-Critic adds 3× LLM cost per task | Budget split + Critic-as-cheapest-model design; metric tracks cost regression | 9 |
| Cross-Worker context contamination via shared memory | MemoryView trait enforces snapshot-at-spawn; private scratchpads quarantined | 9 |
| SLO defaults too tight on weak hardware | Per-tenant SLO overrides via `tenant_settings.slo_*` columns; runbook documents tuning | 10 |
| Burn-rate alert spam on dev environments | Alert routing differentiated dev/prod via tenant tag; documented in S10-2 | 10 |

---

## 6. Sub-agent dispatch plan

Following sprint-7's successful pattern (4 sub-agents, all green):

| Sprint | Sub-agents | What they do in parallel |
|---|---|---|
| **8** | 2 in parallel | A: S8-1 → S8-2 → S8-3 (L3 pipeline). B: S8-5 → S8-6 → S8-7 → S8-8 → **S8-10** (hardening + MiniMax track). I drive S8-4 + S8-9 in main worktree. |
| **9** | 2 in parallel after S9-1 lands | A: S9-2 (Planner). B: S9-3 (Worker). I drive S9-4 + S9-5 + S9-6 + LLDs after both A+B return. |
| **10** | 1 sub-agent | A: S10-2 + S10-3 + S10-4 (alerts + dashboard). I drive S10-1 + S10-5 + S10-6 + S10-7 in main. |

S8-10 (MiniMax) joins the hardening track because it's small (1 day) and
touches `xiaoguai-llm` + `xiaoguai-storage` migrations, which the L3
track doesn't touch.

Disk: 3 worktrees × ~ 30 GB = ~ 90 GB peak. Free space stays ≥ 100 GB
across the sprint (per ci-gotchas budget).

---

## 7. Workflow checkpoint protocol

Per `sprint-workflow.md`, **every sprint follows the 7-step cycle**:

```
1. 更新架构文档    ← done (DEC-019..023 PR in design repo)
2. 安排任务        ← THIS file
3. 审核           ← awaiting user sign-off on this file
4. 执行           ← only after step 3
5. Merge          ← after each sprint's PRs go green
6. 推 git         ← both repos
7. 发 release     ← v1.6.0 after sprint-8 ships; v1.7.0 after sprint-9; v1.8.0 after sprint-10
```

After each sprint's step 7, I write a session handoff + update
`project-status.md` memory + open the next sprint's step 1 PR.

---

## 8. Self-review (6-point protocol)

| # | Check | Result |
|---|---|---|
| 1 | All cited file paths exist | **PASS** — DEC-019..023 land in design repo PR #1; sub-plan files for S8/9/10 will land per task |
| 2 | Every step proposes a runnable verification | **PARTIAL** — table-level; per-task sub-plans need `VC:` lines when written |
| 3 | Each task has a measurable outcome in its sketch | **PASS** — every row has est. + R.E.S.T axis + dependency |
| 4 | Out-of-scope is honored | **PASS** — multi-sprint scope explicit; L4 + Hermes-style swarm explicitly out of this plan |
| 5 | Risks have mitigations | **PASS** — §5 lists 6 cross-sprint risks each with a concrete mitigation |
| 6 | Time estimates are sane | **PASS** — ~ 26.5 dev-days total across 3 sprints = ~ 3 weeks calendar; matches ADR-0020's "~3 weeks for L3 alone" upper bound, and the other two sprints add their own time |

**Soft spots flagged for reviewer**:

1. **S9 is the most uncertain** — multi-agent role design has more degrees of freedom than L3 (which is a known pattern). May need an interactive design session before sub-agent dispatch.
2. **S10 metrics naming** — `xiaoguai_slo_burn_rate{signal,window}` may collide with conventional prometheus-operator alerts. Cross-check before S10-3 lands.
3. **Sprint-8's dual track** (L3 pipeline + hardening track) assumes sub-agents won't step on each other. Files they touch are disjoint (xiaoguai-mcp-exec-wasm + xiaoguai-audit + xiaoguai-mcp/auth + xiaoguai-tasks), but `Cargo.toml` workspace member list is a shared file. Plan: hardening sub-agent gets a small wait-window if L3 sub-agent is editing workspace `Cargo.toml`.
4. **Critic-as-LLM** in DEC-021 is the design call most likely to flip. If Critic ends up being cheaper as a deterministic checker, the topology degenerates back to plan-execute. Sprint-9 sub-plan should benchmark this on real workloads before committing.

---

## 9. Reviewer sign-off log

User signed off on the four reviewer asks on 2026-05-30:

1. ✅ **3-sprint split** approved
2. ✅ **Sprint-8 dual-track dispatch** approved
3. ✅ **Critic-as-LLM** in DEC-021 approved (no feasibility study; ship as-is)
4. ✅ **SLO defaults in DEC-022** approved as starting points (no pre-calibration round)

And added one new requirement:

5. ✅ **MiniMax provider support** — added as DEC-024 and S8-10. User explicitly applied the workflow: "先更新架构文档，再安排任务，再审核，没问题，再执行" — that's what this revision is. The DEC-024 architecture doc lives in `xiaoguai-agent-design#1`; this task plan PR is the corresponding step 2.

## 10. Outstanding asks (this revision)

Awaiting user sign-off on:

1. **S8-10 (MiniMax) added to hardening track** — OK as scoped at 1 day with reasoning_content passthrough + 5-model seed, or want narrower / wider scope?
2. **Total sprint-8 grows from 10 to 11 dev-days** — fits the sprint; flag if you prefer to defer something else to sprint-9 instead.

After your answer I dispatch sub-agents.
