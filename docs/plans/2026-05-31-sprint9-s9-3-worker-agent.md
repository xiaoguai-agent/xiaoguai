# Sprint-9 S9-3 — `WorkerAgent`

**Status:** Sub-plan, drafted 2026-05-29 in worktree
`feat/sprint9-s9-3-worker-agent` (branched off S9-1
`feat/sprint9-s9-1-triangle-scaffolding`). Implements DEC-021 §4.4–4.5
Worker step. Companion to row S9-3 in
`docs/plans/2026-05-31-sprint-9-multi-agent.md`.

---

## 1. Context

S9-1 (PR #82) landed the `triangle/` scaffolding — `Task`, `TaskId`,
`Scratchpad`, `MemorySnapshot`, `Role`, `Verdict`, `TriangleBudget` —
as pure types/traits with no behaviour. S9-3 is the first of the three
sub-agent implementations (alongside parallel S9-2 PlannerAgent).

The Worker's job: take one `Task`, run a full ReAct loop against an LLM
backend, write intermediate state to a per-task `Scratchpad`, and emit a
`WorkerResult` bundling the artefact + citations + self-reported
confidence + cost in tokens.

Critical invariants (from DEC-021 §4.5):

- **Scratchpad quarantine** — Worker writes only to *its* scratchpad,
  keyed by `task_id`. `Scratchpad::append` already enforces this; we
  just have to call it with `task.id`.
- **Budget enforcement at iteration boundaries** — never interrupt
  mid-LLM-call; check token spend between iterations, cancel cleanly
  if exceeded.
- **No state between tasks** — a fresh `ReactAgent` per `execute()`
  call. The WorkerAgent struct holds only configuration (backend,
  persona prompt, allowlist), no run-state.

Driving R.E.S.T axis: **Reliability** — the Critic exists to catch
Worker errors *before* they propagate; the Worker still needs to
honestly report its own confidence so the Critic has the signal.

---

## 2. Success criteria

`cargo test -p xiaoguai-orchestrator` exits 0 with:

- All 27 pre-existing triangle scaffolding tests still pass.
- At least 8 new unit tests in `triangle::worker_agent::tests`:
  1. `happy_completion_one_iteration` — single ScriptStep answer →
     artefact + cost > 0 + iterations == 1 + stop = Completed.
  2. `multi_iteration_three_tool_calls_then_answer` — script with 3
     tool-call steps + final text → iterations == 4 + 4 scratch
     entries.
  3. `budget_exhausted_mid_loop` — small budget; the second iteration
     check fires → stop = BudgetExhausted; artefact = None.
  4. `scratchpad_gets_entry_per_iteration` — exact entry count equals
     `iterations`.
  5. `wrong_task_id_scratchpad_refused` — pass a scratchpad keyed to
     a different task → WorkerError::WrongTaskId, no entries appended.
  6. `confidence_parsed_from_final_message` — final text contains
     `"confidence": 0.85` → result.confidence == 0.85.
  7. `confidence_defaults_to_half_when_absent` — no marker → 0.5.
  8. `citations_extracted_from_urls_and_brackets` — artefact contains
     `https://example.com` and `[1]` markers → both captured.
  9. `max_iterations_stop_reason` — script that keeps producing tool
     calls; WorkerAgent caps via inner AgentConfig (max=2) → stop =
     MaxIterations.

Plus regression: existing 27 triangle tests untouched (verifies the
disjoint-files constraint).

---

## 3. Prerequisites

- S9-1 scaffolding present (`triangle/{plan,scratchpad,memory_view,…}.rs`).
- `xiaoguai-agent` exposing `ReactAgent`, `AgentConfig`, `Toolbox`,
  `AgentOutcome`, `AgentEvent`, `StopReason` — verified at
  `crates/xiaoguai-agent/src/lib.rs`.
- `xiaoguai-llm::MockBackend::with_script` for deterministic
  multi-iteration tests — verified at
  `crates/xiaoguai-llm/src/mock.rs`.
- `tokio-util`, `regex`, `once_cell` available in workspace
  dependencies (verified).

Cargo additions (orchestrator crate):

- `xiaoguai-agent = { path = "../xiaoguai-agent" }`
- `xiaoguai-llm = { path = "../xiaoguai-llm" }`
- `tokio-util = { workspace = true }` (for `CancellationToken`)
- `regex = { workspace = true }` (for citation extraction)
- `once_cell = { workspace = true }` (for compiled-once regex statics)
- `dev-dependency`: nothing new (MockBackend re-exported through
  `xiaoguai-llm` which is now a normal dep).

The `xiaoguai-agent` crate already depends on `xiaoguai-llm`, so adding
both as orchestrator deps does not introduce a cycle (agent→llm,
orchestrator→agent + orchestrator→llm — DAG).

---

## 4. Step-by-step

### Step 4.1 — Wire orchestrator → agent + llm deps

Add the three workspace deps + two path deps to
`crates/xiaoguai-orchestrator/Cargo.toml` (see §3 list).

**VC:** `cargo check -p xiaoguai-orchestrator` exits 0.

### Step 4.2 — Skeleton `triangle/worker_agent.rs`

New file. Public surface:

```rust
pub struct WorkerAgent {
    backend: Arc<dyn LlmBackend>,
    persona_prompt: String,
    tool_allowlist: Vec<String>,
    model: String,         // implementation detail — defaulted in new()
    max_iterations: u32,   // implementation detail — defaulted in new()
}

pub struct WorkerResult { … }       // exactly as spec'd in task brief
pub enum WorkerStopReason { … }     // ditto
pub enum WorkerError { … }          // ditto
```

`new(backend, persona_prompt, tool_allowlist)` defaults `model` to
`"worker"` (the MockBackend ignores model anyway; production wiring
will pass through) and `max_iterations` to 8 (mirrors `AgentConfig`
default). Tests can override via a `with_max_iterations` builder so
the MaxIterations test can pin it to 2.

Add `pub mod worker_agent;` to `triangle/mod.rs` and re-export
`WorkerAgent`, `WorkerResult`, `WorkerStopReason`, `WorkerError`.

**VC:** `cargo build -p xiaoguai-orchestrator` exits 0.

### Step 4.3 — `execute()` body

Algorithm (annotated):

```text
1. Defensive task-id check: try `scratchpad.append(task.id, "[worker started]", None)`.
   - If WrongTask → return WorkerError::WrongTaskId.
   - If EmptyEntry → unreachable (string is non-empty) — map to internal error.
2. Validate budget_tokens >= 1 else return BudgetTooSmall.
3. Build the inner `ReactAgent`:
   - backend = self.backend.clone()
   - toolbox = Toolbox::new() (empty — S9-3 doesn't wire MCP; tests use
     scripted backends that produce tool-call deltas with names that
     won't be in the toolbox → the loop sees a "tool not in toolbox"
     error message and continues. That's deterministic and exercises
     the multi-iteration path without an MCP stub.)
   - config = AgentConfig::new(self.model) with max_iterations =
     self.max_iterations. Temperature defaulted.
4. Build initial messages:
   - Message::system(format!("{persona_prompt}\n\nTask: {}\n\nAcceptance criteria: {}\n\nMemory facts:\n{}",
       task.description, task.acceptance_criteria.rubric, serialize_memory(memory)))
   - (No user message — task instructions are in system. Equivalent
     to the loop_::Agent pattern.)
   ACTUALLY — to follow ReAct convention more cleanly, system holds
   persona + memory; user holds task description + acceptance criteria.
5. Drive via `react.run_stream(initial, cancel)`. Loop on events:
   - On `IterationCompleted { iteration }`:
     - estimate tokens used so far via estimate_message_tokens diff
       against pre-iteration baseline.
     - scratchpad.append(task.id, summary, Some(tokens_delta))
     - if scratchpad.cost_tokens >= budget_tokens: cancel.cancel();
       remember reason = BudgetExhausted.
   - On `Error { message }`: remember reason = ToolError(message);
     cancel.
   - Other events ignored (TextDelta is incremental and not authoritative).
6. await outcome from the join handle. Translate:
   - StopReason::Completed → reason = WorkerStopReason::Completed
   - StopReason::MaxIterations → reason = MaxIterations
   - StopReason::Cancelled → reason = (BudgetExhausted if we cancelled
     for budget; ToolError if for an error; otherwise treat as
     BudgetExhausted defensively).
7. Extract artefact: walk outcome.messages in reverse, find first
   Role::Assistant with non-empty content. None if none found OR if
   reason != Completed.
8. Extract confidence: regex `"confidence"\s*:\s*([0-9.]+)` against
   artefact. Default 0.5; clamp to [0.0, 1.0].
9. Extract citations: union of URLs (regex `https?://[^\s)]+`) and
   bracketed refs (regex `\[\d+\]`). Dedup preserving first-seen order.
10. cost_tokens = scratchpad.cost_tokens (single source of truth).
11. Return WorkerResult { task_id, artefact, citations, confidence,
    cost_tokens, iterations, stop_reason }.
```

Token-delta calc: after each iteration we hold a snapshot of message
count + per-call estimate; the cleanest way is to maintain a `prev_len`
and `prev_tokens` outside the event loop. **However** the event loop
does not have access to the live message vector inside `ReactAgent` —
it only sees events. Pragmatic approach for S9-3: estimate
**per-iteration tokens = estimate_tokens(text_delta_accum) + small
constant for tool call overhead**, by accumulating `TextDelta` deltas
between iteration boundaries. That gives us a non-zero, monotone cost
without requiring ReactAgent surgery.

**VC:** test 4.1–4.4 written + passing.

### Step 4.4 — Confidence + citation extraction helpers

Two free functions (private):

```rust
fn parse_confidence(text: &str) -> f32 { … }    // default 0.5, clamp [0,1]
fn extract_citations(text: &str) -> Vec<String> { … }   // URLs + [\d+]
```

Use `once_cell::sync::Lazy<regex::Regex>` to compile once.

**VC:** tests `confidence_parsed_from_final_message`,
`confidence_defaults_to_half_when_absent`,
`citations_extracted_from_urls_and_brackets` pass.

### Step 4.5 — Unit tests

All 9 tests as enumerated in §2. Use `MockBackend::with_script` to
drive deterministic LLM responses. For the `wrong_task_id` test,
construct a Scratchpad with a foreign id and assert
`WorkerError::WrongTaskId` is returned and `scratchpad.entries()` is
empty.

For the multi-iteration test, the script is:

```rust
vec![
    ScriptStep::tool_calls(vec![ToolCallSpec { id: "1", name: "search", arguments_json: "{}" }]),
    ScriptStep::tool_calls(vec![ToolCallSpec { id: "2", name: "search", arguments_json: "{}" }]),
    ScriptStep::tool_calls(vec![ToolCallSpec { id: "3", name: "search", arguments_json: "{}" }]),
    ScriptStep::text(r#"Final answer "confidence": 0.9 https://example.com"#),
]
```

Empty Toolbox returns "tool not in toolbox" errors for each call; the
ReAct loop continues and eventually emits the final text. 4 iterations.

**VC:** `cargo test -p xiaoguai-orchestrator triangle::worker_agent`
exits 0 with 9 passing tests.

### Step 4.6 — Full crate test sweep

`cargo test -p xiaoguai-orchestrator` — must pass with 27 + 9 = 36
tests minimum (existing 27 + 9 new). Other modules in the crate may
add more.

**VC:** exit code 0, no flakes (run twice).

---

## 5. Out of scope

- PlannerAgent (S9-2 — separate parallel sub-agent).
- CriticAgent (S9-4 — main thread).
- Triangle pattern wiring in `patterns/triangle.rs` (S9-5).
- Real MCP toolbox wiring — S9-3 ships with an empty toolbox; S9-5
  threads through a real one.
- `Plan` round migration to memory (lives in S9-5 orchestrator loop).
- Production token counting via tokenizer (we use the existing
  `estimate_tokens` heuristic).
- Observability metrics (`OrchEvent` emission lives in S9-5).
- HOTL gate wiring (Worker doesn't enforce HOTL directly; the inner
  ReactAgent does, when a gate is configured — S9-3 leaves
  `hotl_gate: None` for simplicity).

---

## 6. Plan adjustment appendix

Empty at draft time. Will be updated if implementation reveals plan
gaps.

---

## 7. Risks

| Risk | Mitigation |
|---|---|
| `ReactAgent` doesn't expose per-iteration tokens, so cost-tracking is approximate | Accumulate `TextDelta` events between iteration boundaries; use `estimate_tokens` on the delta. Better than nothing; production will replace with backend usage reports in a later sprint. Documented as approximation in the module doc-comment. |
| Empty toolbox causes the ReAct loop to log a misleading "tool not in toolbox" tool message that the LLM might surface in the final artefact | This is *the* test mechanism — multi-iteration tests rely on it. Documented inline; S9-5 wires a real toolbox so production won't see this. |
| Cancellation token semantics — `ReactAgent` checks cancellation only at iteration boundaries, so a long-running tool call could blow the budget by hundreds of tokens before we see the next `IterationCompleted` | Acceptable for S9-3; budget enforcement is "best effort between iterations". Worst-case overrun is one iteration's worth, which matches the design intent ("don't interrupt mid-iteration"). |
| Confidence regex picks up `"confidence": 0.85` inside a JSON snippet not intended as the worker's self-report | Pragmatic — the persona prompt should instruct the worker to emit `"confidence": <float>` exactly once at the end. False positives are an LLM-prompting problem, not a parser problem. Documented in the function's doc-comment. |
| `WorkerError::BudgetTooSmall` triggers when `budget_tokens == 0` but tests might want to test "0 budget → exhausted at iteration 0" | Define BudgetTooSmall only for `budget_tokens == 0`; the test for "budget exhausted on iteration 1" uses a small positive budget (e.g. 5). |
| Adding `xiaoguai-agent` + `xiaoguai-llm` as orchestrator deps pulls a large compile graph and slows `cargo test -p xiaoguai-orchestrator` | Both are already compiled by other workspace crates; the marginal cost in incremental builds is ~ 0. Cold build pays once. |

---

## 8. Self-review (6-point protocol)

| # | Check | Result |
|---|---|---|
| 1 | All cited file paths exist | **PASS** — verified `crates/xiaoguai-orchestrator/src/triangle/{mod,plan,scratchpad,memory_view,roles,budget,verdict}.rs`, `crates/xiaoguai-agent/src/{lib,react,toolbox}.rs`, `crates/xiaoguai-llm/src/{mock,types,token_count}.rs`. |
| 2 | Every step proposes a runnable verification | **PASS** — each step ends with a `VC:` line (cargo check/build/test). |
| 3 | Each task has a measurable outcome | **PASS** — §2 enumerates 9 named tests + 27-test regression floor; §4 maps steps to those tests. |
| 4 | Out-of-scope is honored | **PASS** — §5 names S9-2, S9-4, S9-5, real MCP toolbox, observability, HOTL as non-goals. The hard constraint "do NOT touch planner_agent.rs or existing triangle/ files except mod.rs" is honored — only `mod.rs` gets one `pub mod worker_agent;` line + re-exports. |
| 5 | Risks have mitigations | **PASS** — §7 lists 6 risks each with a concrete mitigation. |
| 6 | Time estimates are sane | **PASS** — sprint plan allots 2 dev-days for S9-3. §4 splits into 6 steps each ≤ half-day; matches. |

### Soft spots flagged

1. **Per-iteration token estimation is heuristic.** We track `TextDelta`
   accumulation between iterations. This *under*-counts by the LLM's
   internal reasoning tokens (which we never see in events) and
   *over*-counts when the user prompt itself is long (initial system
   message tokens get attributed to iteration 0). Production needs
   backend `usage` reporting (separate sprint).

2. **Empty Toolbox as test affordance.** The multi-iteration test
   relies on ReactAgent's "tool not in toolbox" error path to drive
   the loop forward. If a future refactor of ReactAgent decides to
   short-circuit unknown tools instead of feeding the error back, the
   test breaks. Flagged — could be defended by an explicit
   `MockToolbox` if the reviewer prefers. Cost: ~30 LOC of test
   scaffolding.

3. **Confidence parsing accepts the first match.** A persona prompt
   that emits multiple `"confidence":` (e.g. in cited JSON) would get
   the wrong one. We take the *last* match instead — final-message
   convention puts the self-report at the end. Documented inline.

4. **`WorkerResult.iterations` semantics.** We surface
   `outcome.iterations` directly from `ReactAgent`, which counts ReAct
   *cycles* (think→act→observe). A terminal "just text" reply still
   counts as one iteration. The scratchpad entry count equals this
   number — one entry per cycle.

5. **No `Clone` on `WorkerAgent`.** `Arc<dyn LlmBackend>` is cheap to
   clone but we don't expose `Clone` because the orchestrator stores
   one WorkerAgent per persona and dispatches sequentially. Easy to
   add later if needed.
