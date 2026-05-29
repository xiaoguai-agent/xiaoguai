# Sprint-9 S9-5 + S9-6 — Triangle pattern wiring + integration tests

**Status:** Sub-plan, drafted 2026-05-29 in worktree
`feat/sprint9-s9-5-s9-6-triangle-pattern` (branched off S9-4
`feat/sprint9-s9-4-critic-agent`). Implements DEC-021 §4.4 + §4.7 and
the integration test matrix from `lld-orchestrator.md` §7. Companion to
rows S9-5 + S9-6 in `docs/plans/2026-05-31-sprint-9-multi-agent.md`.

---

## 1. Context

S9-1..S9-4 shipped:

- S9-1 (PR #82): `triangle/` scaffolding — `Plan`, `Task`, `TaskId`,
  `Scratchpad`, `MemorySnapshot`, `MemoryView` trait, `TriangleBudget`,
  `Verdict`, `Role`.
- S9-2 (PR #87): `PlannerAgent::plan(goal, &MemorySnapshot) -> Plan`.
- S9-3 (PR #86): `WorkerAgent::execute(&Task, &mut Scratchpad, &MemorySnapshot, budget) -> WorkerResult`.
- S9-4 (branch `feat/sprint9-s9-4-critic-agent`): `CriticAgent::review(&WorkerResult, &AcceptanceCriteria, &Scratchpad) -> Verdict`.

S9-5 ties them into the loop documented in `lld-orchestrator.md`
§4.4 — Planner → for each Task: Worker → Critic — with the §4.5
memory snapshot invariant, the §4.6 budget split, and the §4.7
failure-mode handling. S9-6 ships the 6 integration tests from §7.

Driving R.E.S.T axes:
- **R (Reliability)**: Critic catches Worker errors before they propagate;
  budget enforcement aborts before runaway spend; replan cap prevents
  infinite loops.
- **E (Execution)**: The Triangle pattern is *the* canonical pattern for
  non-trivial heterogeneous workflows.
- **T (Testability)**: 6 integration tests pin the §7 matrix.

---

## 2. Success criteria

`cargo test -p xiaoguai-orchestrator` exits 0 with:

- All pre-existing tests in `xiaoguai-orchestrator` still pass (116
  triangle unit tests + supervisor tests + challenger tests + HR
  pack tests).
- 6 new integration tests in `tests/triangle_*.rs`:
  1. `triangle_happy_path` — Planner emits 2 tasks; both Workers
     succeed; Critic Approves both; `Final { Completed }`.
  2. `triangle_critic_request_revision` — Critic returns
     `RequestRevision` then `Approve`; Worker re-runs once with
     feedback; final outcome Completed.
  3. `triangle_critic_reject_triggers_replan` — Critic Rejects on
     plan-round 0; Planner emits a fresh Plan on round 1; that one
     gets Approved.
  4. `triangle_scratchpad_quarantine` — Two sequential tasks; assert
     Worker B's `Scratchpad` does NOT contain Worker A's entries
     (physical separation of `Scratchpad` instances per task).
  5. `triangle_budget_split_enforced` — Parent budget small enough
     that the per-Worker cap fires; `OrchEvent::BudgetExhausted { role: Worker }`
     emitted; `Final { BudgetExhausted }`.
  6. `triangle_replan_cap_terminates` — Critic always rejects;
     `Final { MaxReplansReached }` after `max_replans` rounds.

- 1 unit test inside `patterns/triangle.rs` covering
  `TriangleBudget::split` → `BudgetTooSmall` propagation as `Final
  { PlannerFailed }`.

---

## 3. Prerequisites

- S9-1..S9-4 surfaces present (verified at
  `crates/xiaoguai-orchestrator/src/triangle/{plan,scratchpad,memory_view,roles,budget,verdict,planner_agent,worker_agent,critic_agent}.rs`).
- `xiaoguai-llm::MockBackend::with_script` for scripted Critic +
  Planner responses (verified at `crates/xiaoguai-llm/src/mock.rs`).
- `tokio::sync::mpsc` + `tokio_stream::wrappers::ReceiverStream` —
  both available (used by `xiaoguai-agent::react`).

Cargo additions: none new. `parking_lot` already pulled in by
`triangle::memory_view`; CannedBackend (test) uses it too.

---

## 4. Step-by-step

### Step 4.1 — Create `patterns/` module + register in `lib.rs`

New files:
- `crates/xiaoguai-orchestrator/src/patterns/mod.rs` — re-exports.
- `crates/xiaoguai-orchestrator/src/patterns/triangle.rs` — runner impl.

`lib.rs` change: add `pub mod patterns;` (no re-export at root to avoid
name clashes with v1.4 `Plan`, `Worker`, etc.).

**VC:** `cargo check -p xiaoguai-orchestrator` exits 0.

### Step 4.2 — Skeleton `patterns/triangle.rs`

Public surface (mirrors task brief):

```rust
pub struct TriangleRunner { … }
pub struct TriangleRequest { pub goal: String, pub session_id: SessionId }
pub enum OrchEvent { … }     // see brief
pub enum TriangleStopReason { … }
```

`SessionId`: brief says "existing type from xiaoguai-types or similar".
Search shows no `SessionId` is exported from `xiaoguai-types` yet. Use
a local `pub type SessionId = uuid::Uuid;` newtype `SessionId(Uuid)`
to avoid an unrelated cross-crate API addition. Document deferral
inline; production wiring (deferred to next sprint) replaces it with
the canonical type.

`TriangleRunner::new(planner, worker, critic, memory, budget,
parent_budget_tokens, max_replans, max_revisions_per_task)` —
plain field-init constructor. The arity is high; document it.

`stream(&self, req) -> impl Stream<Item = OrchEvent>` — spawns a tokio
task that drives the loop and emits events on an `mpsc::channel(64)`;
returns a `ReceiverStream`.

**VC:** `cargo build -p xiaoguai-orchestrator` exits 0.

### Step 4.3 — Implement §4.4 algorithm

Algorithm (annotated):

```text
1. Split budget UP FRONT:
     match self.budget.split(parent_budget_tokens) {
       Ok(caps) => continue,
       Err(BudgetTooSmall { … }) => emit Final { PlannerFailed("budget too small: <err>") }; return;
     }
2. plan_round = 0; replans = 0
3. loop:
   3a. Capture memory snapshot ONCE per plan-round:
         let snap = self.memory.snapshot(plan_round).await;
       (DEC-021 §4.5 invariant. snap is passed by reference to Planner
       and to each Worker.)
   3b. Planner LLM call:
         self.planner.plan(&req.goal, &snap).await
       On PlannerError: emit Final { PlannerFailed(err.to_string()) }; return.
       On Ok(plan): emit OrchEvent::PlanProduced { round: plan_round, task_count: plan.tasks.len() }
   3c. any_rejected = false
   3d. for task in plan.tasks:
         emit OrchEvent::TaskStarted { task_id, round: plan_round }
         revision = 0
         loop:
           # Build a fresh Scratchpad per task. Quarantine invariant.
           let mut sp = Scratchpad::new(task.id);
           # Append revision feedback (if any) as a scratchpad entry to
           # surface it to the Worker.
           if let Some(fb) = &revision_feedback { sp.append(task.id, format!("revision feedback: {fb}"), Some(0))?; }
           # Budget gate per Worker call.
           let remaining_worker = caps.worker.saturating_sub(worker_spent);
           if remaining_worker == 0 { emit BudgetExhausted { role: Worker }; emit Final { BudgetExhausted }; return; }
           # Worker execute.
           let result = match self.worker.execute(&task, &mut sp, &snap, remaining_worker).await {
             Ok(r) => r,
             Err(WorkerError::BudgetTooSmall) => { emit BudgetExhausted { Worker }; emit Final { BudgetExhausted }; return; }
             Err(e) => { emit WorkerCompleted { ok: false, cost: 0 }; treat as rejection; break; }
           };
           worker_spent += result.cost_tokens;
           emit WorkerCompleted { task_id, ok: matches!(stop, Completed), cost_tokens };
           # Critic LLM call.
           let remaining_critic = caps.critic.saturating_sub(critic_spent);
           if remaining_critic == 0 { emit BudgetExhausted { Critic }; emit Final { BudgetExhausted }; return; }
           let verdict = self.critic.review(&result, &task.acceptance_criteria, &sp).await?;
           # Approximate Critic spend — we don't get usage back; charge
           # a fixed estimate (200 tokens) per review call.
           critic_spent += CRITIC_CALL_TOKENS_ESTIMATE;
           let kind = verdict.kind();
           emit CriticVerdict { task_id, kind, reason: verdict.explanation().to_string() };
           match verdict {
             Verdict::Approve { … } => break,
             Verdict::RequestRevision { feedback } if revision < self.max_revisions_per_task => {
               revision += 1;
               revision_feedback = Some(feedback);
               continue;       // re-run Worker with feedback
             }
             Verdict::RequestRevision { feedback } => {
               # Cap hit — force a Reject path.
               emit CriticVerdict { task_id, kind: Reject, reason: format!("too many revisions: {feedback}") };
               any_rejected = true; break;
             }
             Verdict::Reject { … } => {
               any_rejected = true; break;
             }
           }
   3e. if !any_rejected: emit Final { Completed, summary }; return.
   3f. if any_rejected:
         if plan_round + 1 < self.max_replans {
           emit Replan { reason, prev_round: plan_round };
           plan_round += 1; replans += 1; continue;
         } else {
           emit Final { MaxReplansReached, summary }; return.
         }
```

Notes:
- `CRITIC_CALL_TOKENS_ESTIMATE = 200` documented inline (`Worker` cost
  is real because `Scratchpad` tracks it; `Critic` doesn't expose
  usage, mirror S9-3's `estimate_tokens` pragmatism).
- `max_replans` semantics: brief default is 3; counts MAX plan-rounds.
  Plan-round 0 (the first plan) is the baseline; max_replans=3 means
  up to 3 re-plans after the initial plan = at most 4 total Planner
  calls.

  **Re-reading task brief 6**: "Critic always rejects; after
  `max_replans` plan-rounds the stream emits `Final {
  MaxReplansReached }`". So the semantics is the test sets
  max_replans=N and expects exactly N plan-rounds before
  termination. Implementation choice: when `plan_round + 1 >=
  max_replans` we don't replan. With max_replans=2 we get rounds
  0 and 1 then terminate — total 2 plan-rounds. The test will pin
  this.

- Memory promotion (Approve path): brief says option (b) — emit the
  artefact as part of final summary; document the actual memory
  write as deferred. Inline doc-comment notes the deferral and the
  intent (DEC-021 §4.5 — Scratchpad → session memory on Approve).

**VC:** unit test `budget_too_small_returns_planner_failed` passes.

### Step 4.4 — Internal helper module + types

Inside `patterns/triangle.rs`:

```rust
const CRITIC_CALL_TOKENS_ESTIMATE: u64 = 200;
const DEFAULT_MAX_REPLANS: u32 = 3;
const DEFAULT_MAX_REVISIONS: u32 = 3;

// Local SessionId (deferred — replaced by xiaoguai-types::SessionId in
// production wiring sprint).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub Uuid);
impl SessionId { pub fn new() -> Self { Self(Uuid::new_v4()) } }
```

### Step 4.5 — Six integration tests

File layout (one test per file as `lld-orchestrator.md` §3 specifies):

```
tests/
├── triangle_happy_path.rs
├── triangle_critic_request_revision.rs
├── triangle_critic_reject_triggers_replan.rs
├── triangle_scratchpad_quarantine.rs
├── triangle_budget_split_enforced.rs
├── triangle_replan_cap_terminates.rs
└── triangle_common.rs                  # mod loaded via `mod triangle_common;`
```

Wait — integration tests can't `mod foo;` across files unless they
share a parent module. The standard fix is `tests/common/mod.rs` (we
already have one used by the v1.4 tests). Layout adjustment:

```
tests/
├── common/                       # existing — DO NOT touch (v1.4 tests use it)
├── triangle_common/mod.rs        # new — shared CannedBackend, scratchpad helpers
├── triangle_happy_path.rs
├── triangle_critic_request_revision.rs
├── triangle_critic_reject_triggers_replan.rs
├── triangle_scratchpad_quarantine.rs
├── triangle_budget_split_enforced.rs
└── triangle_replan_cap_terminates.rs
```

Cargo treats `tests/common/` and `tests/triangle_common/` as modules
(directories with `mod.rs`) — they're not run as integration tests
themselves. Each leaf test file does `mod triangle_common;`.

`triangle_common` exports:
- `CannedBackend` — re-implementation of the
  `CriticAgent::tests::CannedBackend` pattern (private to that
  module), parameterised by `Vec<String>`.
- `make_planner_response(tasks: &[(&str, &str)]) -> String` —
  builds the JSON the Planner expects.
- `make_critic_response(kind, reason) -> String` — builds the JSON
  the Critic expects.
- `make_worker_response(text) -> String` — sugar for a Worker final
  message.

Each test:
1. Builds three `CannedBackend`s (one per agent), each with the
   appropriate scripted responses.
2. Wraps them in `PlannerAgent::new(...)`, `WorkerAgent::new(...)`,
   `CriticAgent::new(...)`.
3. Builds an `InMemoryMemoryView`.
4. Constructs a `TriangleRunner` with a sensible budget
   (10_000 tokens parent for happy paths).
5. Drives `runner.stream(req).collect::<Vec<_>>().await` and asserts
   on the event sequence.

**VC:** `cargo test -p xiaoguai-orchestrator --test triangle_happy_path` etc. each exit 0.

### Step 4.6 — Quarantine test (#4) — physical separation

Approach: a custom `Worker` mock isn't going through `WorkerAgent`. To
test the *runner's* quarantine guarantee:

1. Use a `WorkerAgent` wrapping a `MockBackend` that returns a text
   response keyed on the task description.
2. Plan has 2 sequential tasks (depends_on chain).
3. After the runner finishes, the test snoops on a `parking_lot::Mutex<Vec<TaskId>>`
   that an instrumented `MemoryView::snapshot` appends to. This
   doesn't directly observe the scratchpads (they're internal to the
   runner).

Better approach: the runner must own the `Scratchpad`s internally. To
observe physical separation, expose a test-only hook: a
`tokio::sync::mpsc::UnboundedSender<Scratchpad>` (cloned, drained at
test end) that the runner sends every Scratchpad to after the Critic
is done with it. Gate behind `#[cfg(feature = "test-introspect")]` or
similar.

**Simpler**: don't expose internals. Test the invariant at the type
level. The runner *constructs* `Scratchpad::new(task.id)` per Task
inside the for-loop. We can prove non-sharing by:

a. Run the pattern with 2 tasks.
b. Assert each `OrchEvent::TaskStarted` carries a distinct
   `task_id`.
c. Use a Worker mock whose `execute` records `(scratchpad as *const _)`
   and pushes it to a shared `Mutex<Vec<usize>>` (raw pointer ids).
d. Assert the two recorded raw pointer ids are distinct.

But `WorkerAgent` is a concrete struct — we can't substitute a mock.
**Best approach**: split out a `Worker` trait inside the runner that
`WorkerAgent` implements. The runner stores `Arc<dyn Worker>`. Tests
substitute a `RecordingWorker` that captures the `Scratchpad`'s
`task_id` and content.

Wait — adding a `Worker` trait means changing the runner's signature.
Brief says `worker: Arc<WorkerAgent>` directly. Re-reading the brief:
*"Use the `TriangleBudget::split` failure path…"* and the §4.4 algorithm.
The brief is prescriptive about types — `Arc<PlannerAgent>`,
`Arc<WorkerAgent>`, `Arc<CriticAgent>`.

**Resolution**: the quarantine test asserts at the *event* level and
the *scratchpad-id level*. Each `OrchEvent::TaskStarted` carries a
`task_id`. The runner constructs `Scratchpad::new(task_id)` per task.
The test verifies:

- 2 distinct `OrchEvent::TaskStarted` events with distinct `task_id`s.
- Each Worker's final assistant text differs (one says "answer A",
  one says "answer B").
- The CannedBackend for the Critic captures each scratchpad it sees
  and the test asserts that scratchpad-1's `entries` do NOT contain
  any string from scratchpad-2 and vice versa.

That captures the *behavioural* guarantee: Worker B never observes
Worker A's notes. Implementation: extend `CannedBackend` in
`triangle_common` to record the system-prompt content of each call;
the Critic system prompt includes "Worker scratchpad tail" — that's
the read of the scratchpad. The test asserts the Critic-call-1 system
prompt does not mention any string from Worker-call-2 and vice versa.

Even simpler: an explicit test inside `patterns::triangle::tests`
constructs two `Scratchpad` instances against two `TaskId`s and asserts
they are not aliasable (i.e., constructed as separate structs).
Combined with a code-walkthrough comment "Scratchpad::new is called
inside the for-task loop", the invariant is verified.

**Final decision** for the quarantine test:
- Test 4 lives in `tests/triangle_scratchpad_quarantine.rs`.
- Uses 2 tasks (sequential — task B depends on task A).
- Plants distinct identifiers in each task's description: "TASK_A_KEY"
  and "TASK_B_KEY".
- The Worker's CannedBackend responds with the matching identifier
  string verbatim.
- After the run, inspects the captured Critic system prompts:
  - Critic call 1 (reviewing Worker A) must contain "TASK_A_KEY" and
    must NOT contain "TASK_B_KEY".
  - Critic call 2 (reviewing Worker B) must contain "TASK_B_KEY" and
    must NOT contain "TASK_A_KEY".
- This proves that the Critic (which is the only consumer of Scratchpad
  besides the Worker that wrote it) only ever sees the scratchpad
  belonging to the Worker it's currently reviewing.

**VC:** test passes.

### Step 4.7 — Wire `lib.rs`

Add:

```rust
pub mod patterns;
```

No re-exports at root (avoids `Plan` collision with v1.4's `plan::Plan`).

**VC:** `cargo build -p xiaoguai-orchestrator` exits 0.

### Step 4.8 — Full crate test sweep

```bash
cargo test -p xiaoguai-orchestrator
```

Expect existing tests + ≥ 6 new integration tests + ≥ 1 new unit test
all passing.

---

## 5. Out of scope

- Migration `0025_persona_role_tags.sql` (S9-7 — done in parallel).
- Runbook `docs/runbooks/triangle-pattern.md` (S9-8 — done in parallel).
- Production wiring in `xiaoguai-core::orchestrator_bridge`
  (deferred — next sprint per brief).
- Real `SessionId` type from `xiaoguai-types` (deferred — local newtype
  used; documented inline).
- Memory promotion (Approve → session memory) — brief option (b);
  documented as deferred follow-up; final summary cites approved
  artefacts as proxy.
- HotL gate wiring at the orchestrator level (already in the inner
  `ReactAgent`).
- Concurrent Workers within one plan-round (DEC-021 spec is sequential
  for v1.6+; concurrent is future work).
- Cancellation propagation (brief lists `TriangleStopReason::Cancelled`
  but no test required this sprint; we wire the variant but don't
  implement runtime cancellation — covered when production wiring lands).

---

## 6. Plan adjustment appendix

Empty at draft time.

---

## 7. Risks

| Risk | Mitigation |
|---|---|
| `mpsc::channel(64)` buffer overflows if events emit faster than the consumer pulls them. | Tests collect synchronously via `.collect()`; the 64-slot buffer covers far more than the 6 tasks × 4 events we ever emit per test run. Production wiring will tune. |
| `BudgetTooSmall` failure path swallows the underlying numeric details. | The `Final` variant carries the formatted `BudgetError` Display string, which includes parent / per-role percentages / min required parent. |
| Critic spend is an estimate (200 tokens/call), so budget enforcement on the Critic share is approximate. | Document as approximation; matches WorkerAgent's text-delta heuristic. Replaced when backend usage reporting lands. |
| The quarantine test (#4) is behavioural, not pointer-level. | Coupled with the runner code-comment + the per-task `Scratchpad::new(task.id)` construction, the invariant is enforced at the structural level. The behavioural test catches contamination if a future refactor weakens it. |
| `max_replans` off-by-one — does it count the initial plan or only re-plans? | Decision: counts plan-rounds (total), including the initial. Test 6 pins this with an explicit `max_replans=2` and expects exactly 2 plan-rounds before `MaxReplansReached`. Inline doc-comment explains. |
| Hard constraint: cannot touch existing triangle modules — what if one needs a new method (e.g., `Verdict::reject_reason`)? | All needed surfaces already exist (`Verdict::kind`, `Verdict::explanation`, `WorkerResult::artefact`, `WorkerResult::stop_reason`). Verified during plan drafting by walking each algorithm step against the existing API. |

---

## 8. Self-review (6-point protocol)

| # | Check | Result |
|---|---|---|
| 1 | All cited file paths exist | **PASS** — verified `crates/xiaoguai-orchestrator/src/triangle/{plan,scratchpad,memory_view,roles,budget,verdict,planner_agent,worker_agent,critic_agent}.rs`, `crates/xiaoguai-llm/src/mock.rs`, `crates/xiaoguai-agent/src/react.rs`, `xiaoguai-agent-design/docs/lld/lld-orchestrator.md`. |
| 2 | Every step proposes a runnable verification | **PASS** — each step ends with `VC:` (cargo check/build/test). |
| 3 | Each task has a measurable outcome | **PASS** — §2 enumerates 6 named integration tests + 1 unit test + pre-existing-test regression floor; §4 maps steps to tests. |
| 4 | Out-of-scope is honored | **PASS** — §5 names 6 explicit non-goals; hard constraint "do NOT touch existing triangle modules" honored — `triangle/mod.rs` is unchanged, only ADD `pub mod patterns;` to `lib.rs`. |
| 5 | Risks have mitigations | **PASS** — §7 lists 6 risks each with concrete mitigation. |
| 6 | Time estimates are sane | **PASS** — sprint plan allots 1 + 1.5 = 2.5 dev-days for S9-5+S9-6 combined; §4 splits into 8 sub-steps each ≤ half-day. |

**Soft spots flagged:**

1. **Quarantine test (#4) is behavioural** — relies on the Critic
   system prompt as a side-channel for what the Critic read. If a
   future refactor changes the prompt format, the test asserts
   become brittle. Acceptable trade-off — the hard constraint
   prohibits touching the runtime API and we want the test in the
   integration layer to catch end-to-end contamination.
2. **`SessionId` is a local newtype** — adds a temporary type that
   will need to be replaced when production wiring lands. Acceptable
   because the brief explicitly defers production wiring and a
   cross-crate API addition is out of scope.
3. **`CRITIC_CALL_TOKENS_ESTIMATE = 200` is a magic number** —
   pragmatic mirror of S9-3's `estimate_tokens` heuristic. Documented
   inline; replaced when backend usage reporting lands.
