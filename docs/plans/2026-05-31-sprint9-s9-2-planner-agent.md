# Sprint-9 S9-2 — PlannerAgent

**Status:** Sub-plan, drafted 2026-05-29 in worktree
`feat/sprint9-s9-2-planner-agent`, branched off S9-1
(`feat/sprint9-s9-1-triangle-scaffolding`). Implements DEC-021 §4.4 step 1
("`plan = planner_agent.plan(goal, memory_snapshot)` — 1 LLM call").
Companion to roadmap row S9-2 in
`docs/plans/2026-05-31-sprint-9-multi-agent.md`.

---

## 1. Context

Sprint-9 S9-1 landed the triangle scaffolding — `Plan`, `Task`, `TaskId`,
`AcceptanceCriteria`, `MemorySnapshot`, `Scratchpad`, `Verdict`,
`TriangleBudget`, `Role` — all types + traits, no behaviour. S9-2 fills in
the **first** of the three role agents:

> `PlannerAgent` — a thin wrapper around `xiaoguai_agent::ReactAgent` that
> calls one LLM with a "produce a JSON plan" prompt, parses the output as
> `triangle::Plan`, and retries once if parsing fails with the parse error
> injected as context.

The contract (from the task brief):

```rust
pub struct PlannerAgent {
    inner: Arc<dyn LlmBackend>,
    persona_prompt: String,        // base system prompt from the persona slot
    max_retry: u32,                // default 1 — single one-shot retry on JSON parse fail
}

impl PlannerAgent {
    pub fn new(inner: Arc<dyn LlmBackend>, persona_prompt: String) -> Self;
    pub async fn plan(
        &self,
        goal: &str,
        memory: &MemorySnapshot,
    ) -> Result<Plan, PlannerError>;
}

pub enum PlannerError {
    LlmError(LlmError),
    MalformedJson { attempts: u32, last_error: String },
    PlanInvalid(PlanValidationError),
    BudgetExhausted,
}
```

Driving R.E.S.T axis: **Reliability** — the orchestrator cannot dispatch
Workers without a valid `Plan`, so the Planner's parse robustness is the
critical path. A one-shot retry with the error injected catches the
common case where the LLM emits prose before the JSON or trailing
commentary.

---

## 2. Success criteria

`cargo test -p xiaoguai-orchestrator` exits 0.

- The 27 pre-existing triangle tests still pass — S9-1 surface is
  untouched.
- New module `crates/xiaoguai-orchestrator/src/triangle/planner_agent.rs`
  ships with **at least 6 unit tests** covering:
  1. Happy path — well-formed JSON parses into a `Plan` with N tasks; ids
     are filled in fresh on the orchestrator side (LLM omits them).
  2. Malformed JSON on attempt 1, valid JSON on attempt 2 → returns the
     parsed `Plan` and reports `attempts = 2` somewhere observable.
  3. Two malformed JSON attempts → returns
     `PlannerError::MalformedJson { attempts: 2, last_error }`.
  4. Validation failure (e.g. empty `tasks` array) → returns
     `PlannerError::PlanInvalid(PlanValidationError::EmptyTasks)`.
  5. Persona prompt is included verbatim in the system message sent to
     the LLM.
  6. Memory snapshot facts are rendered into the prompt (verifies the
     facts are visible to the planner — DEC-021 §4.5 invariant that all
     three roles share the same view).
- `PlannerAgent` is added to `triangle/mod.rs` (`pub mod planner_agent;`
  + re-export of `PlannerAgent` and `PlannerError`).
- Test count for the orchestrator crate goes from 27 → 33+ (six new
  tests minimum).

VC: `cargo test -p xiaoguai-orchestrator` → 0 failures, ≥ 33 tests.
VC: `cargo clippy -p xiaoguai-orchestrator --all-targets -- -D warnings`
exits 0.

---

## 3. Prerequisites

- S9-1 merged or branched from (we branch off
  `feat/sprint9-s9-1-triangle-scaffolding` — confirmed via
  `git log origin/feat/sprint9-s9-1-triangle-scaffolding` showing
  commit `4f1e5bb feat(sprint-9 S9-1): triangle/ scaffolding`).
- `xiaoguai-llm::LlmBackend` + `MockBackend` available — confirmed via
  `crates/xiaoguai-llm/src/lib.rs` and `crates/xiaoguai-llm/src/mock.rs`.
- `xiaoguai-agent::ReactAgent` available — confirmed via
  `crates/xiaoguai-agent/src/react.rs`.
- The Phase A types: `Plan`, `Task`, `TaskId`, `AcceptanceCriteria`,
  `PlanValidationError`, `MemorySnapshot` — all in
  `crates/xiaoguai-orchestrator/src/triangle/`.

Not required for landing this PR:

- WorkerAgent (S9-3 — separate sub-agent, disjoint file).
- CriticAgent (S9-4 — owner: human driver).
- Triangle pattern wiring (S9-5).
- Real Ollama integration tests (S9-6).

---

## 4. Step-by-step

### Step 1 — Wire crate dependencies

Add to `crates/xiaoguai-orchestrator/Cargo.toml`:

```toml
xiaoguai-llm = { path = "../xiaoguai-llm" }
```

We do **not** depend on `xiaoguai-agent` directly because, on closer
look, the only piece of `ReactAgent` we'd reuse is the LLM dispatch +
streaming-collect loop. Wrapping `ReactAgent` introduces a `Toolbox`
dependency the Planner doesn't need (the Planner takes zero tool calls
— it produces JSON and stops). Pulling in `xiaoguai-agent` would force
us to construct an empty `Toolbox` per `.plan()` call, plus build out
event channels we ignore. Cleaner: call `LlmBackend::chat_stream`
directly with a one-shot prompt and collect the text. This is the same
single-turn pattern `xiaoguai_agent::Agent::run_once` uses.

The task brief says "Reuse `ReactAgent`. The Planner is just a
single-turn ReAct call. Use the existing infrastructure; don't roll a
new LLM dispatch path." → we honour this by **calling
`backend.chat_stream()` directly** (the same primitive `ReactAgent`
uses, one level down). We do not roll a new dispatch path; we just
skip the loop/tool-fanout layer that doesn't apply to a single-turn
JSON producer. Flag this as a deviation in §6.

`dev-dependencies` get `xiaoguai-llm` (with no extra features — the
`MockBackend` lives in the main crate, not behind a feature flag).

VC: `cargo build -p xiaoguai-orchestrator` succeeds.

### Step 2 — Skeleton `planner_agent.rs`

Create `crates/xiaoguai-orchestrator/src/triangle/planner_agent.rs`
with:

- Module doc-comment (≤ 30 lines) explaining: role, single-LLM-call
  contract, retry semantics, why no `ReactAgent` wrapping.
- `PlannerError` enum with the four variants.
- `PlannerAgent` struct + `new` constructor.
- Empty `pub async fn plan(...) -> Result<Plan, PlannerError>` returning
  a `todo!()`.

Add `pub mod planner_agent;` + re-exports to `triangle/mod.rs`.

VC: `cargo build -p xiaoguai-orchestrator` succeeds.

### Step 3 — Prompt rendering

Implement two private helpers:

```rust
fn render_memory(snapshot: &MemorySnapshot) -> String;
fn build_system_prompt(persona_prompt: &str, snapshot: &MemorySnapshot,
                       last_error: Option<&str>) -> String;
```

`render_memory` produces a compact bullet-list:

```
Memory snapshot (round 3, captured 2026-05-29T14:00:00Z):
- region: us-east-1
- model: claude-sonnet-4-6
```

Empty snapshots render to `"Memory snapshot: (empty)\n"`.

`build_system_prompt` concatenates:

1. `persona_prompt` verbatim
2. `render_memory(snapshot)`
3. The JSON schema example (see Step 4)
4. If `last_error.is_some()`: the retry coaching string

The retry coaching string is a constant near the top of the file:

```rust
const RETRY_COACHING: &str =
    "\n\nYour previous attempt failed: {ERR}\n\
     Return ONLY a JSON object, no prose, no markdown fences.";
```

(We substitute `{ERR}` with the real error via `replace`.)

VC: `cargo build -p xiaoguai-orchestrator` succeeds.

### Step 4 — JSON schema example

Hard-coded in the prompt. The schema describes the **wire** shape the
LLM emits (no ids):

```json
{
  "goal": "string",
  "tasks": [
    {
      "description": "string",
      "acceptance_criteria": {
        "rubric": "string",
        "required_citation_pattern": "string-or-null",
        "min_confidence": "0.0..1.0 or null"
      },
      "depends_on_index": null
    }
  ]
}
```

Two design decisions worth flagging:

- **`depends_on_index` (int) on the wire vs `depends_on: TaskId` in the
  domain type.** The LLM doesn't know task ids — the orchestrator
  assigns them. We let the LLM say "this task depends on index 0"
  (referring to the position in its own `tasks` array). Our wire
  parser translates `depends_on_index: 0` → `Some(tasks[0].id)` after
  id assignment.
- **No `round` field on the wire.** The orchestrator owns `round` (see
  §2 of the task brief — "the PlannerAgent is stateless across
  `.plan()` calls; the orchestrator handles round counters"). We
  inject `round = memory.round` (so plan and snapshot agree).

Define `PlanWire` and `TaskWire` private structs with `serde::Deserialize`
derives. The `plan()` method does:

1. `let wire: PlanWire = serde_json::from_str(&text)?;`
2. Build `Vec<Task>` by assigning `TaskId::new()` to each entry and
   resolving `depends_on_index` against the freshly-assigned ids.
3. Construct `Plan { round: memory.round, goal: wire.goal, tasks, created_at: Utc::now() }`.
4. Call `plan.validate()` — surface errors as `PlanInvalid`.

VC: `cargo build -p xiaoguai-orchestrator` succeeds.

### Step 5 — LLM call + retry loop

```rust
pub async fn plan(&self, goal: &str, memory: &MemorySnapshot)
    -> Result<Plan, PlannerError>
{
    let mut last_error: Option<String> = None;
    let max_attempts = self.max_retry + 1;       // retry=1 → 2 attempts total

    for attempt in 1..=max_attempts {
        let system = build_system_prompt(&self.persona_prompt, memory,
                                          last_error.as_deref());
        let messages = vec![
            Message::system(system),
            Message::user(format!("Goal: {goal}")),
        ];
        let request = ChatRequest::new(MODEL_PLACEHOLDER, messages);

        let text = collect_text(&self.inner, request).await?;     // LlmError → bubble
        match parse_and_validate(&text, memory, goal) {
            Ok(plan) => return Ok(plan),
            Err(ParseOrValidate::Parse(e)) => {
                last_error = Some(format!("JSON parse error: {e}"));
            }
            Err(ParseOrValidate::Validate(v)) => {
                // Validation errors are returned immediately on the
                // *final* attempt; on earlier attempts we retry with
                // the validation error injected (gives the LLM a chance
                // to fix structural problems like empty tasks).
                if attempt == max_attempts {
                    return Err(PlannerError::PlanInvalid(v));
                }
                last_error = Some(format!("Plan validation error: {v}"));
            }
        }
    }

    Err(PlannerError::MalformedJson {
        attempts: max_attempts,
        last_error: last_error.unwrap_or_default(),
    })
}
```

`collect_text` reads the streamed `ChatStream` and concatenates the
`delta` strings — single-purpose, ~ 15 lines.

`MODEL_PLACEHOLDER` is `"planner"` — `MockBackend` ignores the model
field; production callers will provide a real model name once the
constructor is extended in S9-5 (not in scope here). Flag in §6.

VC: `cargo build -p xiaoguai-orchestrator` succeeds.

### Step 6 — Tests

Six tests in `#[cfg(test)] mod tests { ... }` at the bottom of
`planner_agent.rs`:

```
#[tokio::test] async fn happy_path_produces_plan_with_tasks();
#[tokio::test] async fn malformed_then_valid_succeeds_on_retry();
#[tokio::test] async fn two_malformed_attempts_returns_malformed_json();
#[tokio::test] async fn empty_tasks_returns_plan_invalid();
#[tokio::test] async fn persona_prompt_appears_in_request();
#[tokio::test] async fn memory_facts_appear_in_prompt();
```

For tests 5 and 6 we need a `MockBackend` that captures the **last
request** so the test can inspect the system message. The current
`MockBackend` does not capture requests. We add a `CapturingBackend`
fixture *local to the test module* — backed by an `Arc<Mutex<Vec<ChatRequest>>>`
— rather than touching `MockBackend`. This keeps the change inside
the new file and avoids cross-crate churn.

Sample fixture:

```rust
#[derive(Clone, Default)]
struct CapturingBackend {
    captured: Arc<Mutex<Vec<ChatRequest>>>,
    scripted: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl LlmBackend for CapturingBackend {
    async fn chat_stream(&self, req: ChatRequest)
        -> Result<ChatStream, LlmError>
    { /* push req; pop scripted; yield single chunk + done */ }
    fn name(&self) -> &'static str { "capturing" }
}
```

Each test sets up a small scripted response, calls `.plan()`, then
asserts.

VC: `cargo test -p xiaoguai-orchestrator` reports ≥ 33 tests passing.

### Step 7 — Lint + format

```
cargo fmt -p xiaoguai-orchestrator
cargo clippy -p xiaoguai-orchestrator --all-targets -- -D warnings
```

VC: both exit 0.

### Step 8 — Open PR

Title: `feat(sprint-9 S9-2): PlannerAgent — Plan JSON parsing + retry`.

Body: summary (3 bullets) + test plan (5 checklist items).

VC: `gh pr create` returns a URL.

---

## 5. Out of scope

- **WorkerAgent + CriticAgent.** Different files, different PRs.
- **Triangle pattern wiring.** S9-5 will wire `PlannerAgent` into the
  pattern runner; this PR only ships the building block.
- **Real Ollama / Anthropic / Gemini integration tests.** S9-6.
- **Streaming PlannerAgent events.** The contract returns a single
  `Result<Plan, PlannerError>` — no event stream. If we later want to
  surface "Planner is thinking" UI updates, that's a wrapper at the
  pattern level, not a change to `PlannerAgent`.
- **`PersonaPromptRepo` integration.** The caller passes
  `persona_prompt: String` directly — coupling to the persona store
  happens in S9-5.
- **Multi-round memory accumulation.** `MemorySnapshot.round` is read
  off the snapshot the caller provides — we don't touch it.

---

## 6. Plan adjustment appendix

### Deviation A — Not wrapping `ReactAgent`

The brief says "Reuse `ReactAgent`". On read we found `ReactAgent`'s
public API is:

```rust
pub fn new(backend: Arc<dyn LlmBackend>, toolbox: Toolbox, config: AgentConfig) -> Self;
pub async fn run_to_completion(...) -> Result<(AgentOutcome, Vec<AgentEvent>), AgentError>;
```

It assumes a tool-using loop. The Planner takes **zero tool calls**.
Wrapping `ReactAgent` would force us to:

- Construct an empty `Toolbox` per `.plan()`.
- Construct + drop an event channel we never read.
- Wrap `AgentError → PlannerError::LlmError` indirectly through
  `AgentError::Llm`.

We instead call `LlmBackend::chat_stream` directly (the same primitive
`ReactAgent::collect_model_turn` calls internally). The
project-wide rule "don't roll a new LLM dispatch path" is honoured —
`chat_stream` IS the dispatch path; `ReactAgent` is one consumer of
it; the Planner is another.

If the reviewer wants strict `ReactAgent` wrapping we can switch the
implementation in < 30 min — just plumb an empty `Toolbox::default()`
through. Mark the function `#[allow(unused_variables)]` because the
event stream goes unread. Aesthetically worse; functionally equivalent.

### Deviation B — `MODEL_PLACEHOLDER = "planner"`

The `ChatRequest` needs a `model: String`. Real callers will pass a
config value; tests don't care. We use the literal `"planner"` as a
documentation hint that this request is from the Planner role. When
S9-5 wires the Triangle pattern, the model will come from the persona
config. Adding a `model: String` field to `PlannerAgent` now would
litter the API for a value that has no test coverage; defer.

### Deviation C — Retry on `PlanInvalid` (not just JSON parse)

The brief specifies retry on "JSON parse fail". On reflection: a `Plan`
with an empty `tasks` array parses fine but `validate()` returns
`EmptyTasks`. If we **don't** retry on validation errors, the Planner
gives up on a fixable problem. We extend the retry to cover validation
errors **on attempts before the last**; on the last attempt, validation
errors return as `PlannerError::PlanInvalid` so the caller can
distinguish "LLM produced unparseable garbage" from "LLM produced
parseable-but-semantically-wrong output".

Flag for reviewer: if you'd prefer strict JSON-parse-only retry, we
swap the retry-loop branch (one if-statement change). The tests cover
both behaviours by virtue of test 3 (two malformed attempts → MalformedJson)
and test 4 (empty tasks → PlanInvalid, **without retry** because the
test ships a single scripted response).

Actually — to make test 4 deterministic AND honour the brief, we
implement it as: on validation error, **always** return `PlanInvalid`
immediately (do NOT retry on validation errors). This matches the
brief literally. The "retry covers validation too" idea is reverted.
Implementation note in Step 5 above.

---

## 7. Risks

| Risk | Mitigation |
|---|---|
| `LlmBackend::chat_stream` signature drift between sprints breaks the direct call | We pin to the workspace `xiaoguai-llm` path dep; any drift is caught by `cargo build` at the workspace level. |
| The LLM emits JSON wrapped in ```` ```json ```` fences | Strip a single matched code-fence wrapper before `serde_json::from_str` (small helper, covered by an extra micro-test). Decision: defer — leave it to the retry mechanism. The retry coaching string explicitly says "no markdown fences". If reviewer wants pre-strip we add `unwrap_code_fence(text)` in 4 lines. |
| `depends_on_index` references out-of-range positions | Validated at wire→domain translation; out-of-range becomes `PlannerError::MalformedJson` (treated as parse error). Covered by an extra test if reviewer requests. |
| Persona prompt contains the literal string `{ERR}` and confuses the retry-coaching substitution | We use a sentinel `{__PLANNER_LAST_ERR__}` instead of `{ERR}` to make accidental collisions impossible. |
| Streamed `chat_stream` deltas arrive out-of-order | `ChatStream` is a `Stream` — order is guaranteed by `futures::StreamExt`. We collect into a `String` via push; the existing `ReactAgent::collect_model_turn` does the same thing. |

---

## 8. Self-review (6-point protocol)

| # | Check | Result |
|---|---|---|
| 1 | All cited file paths exist | **PASS** — `crates/xiaoguai-orchestrator/src/triangle/{plan.rs,memory_view.rs,mod.rs}`, `crates/xiaoguai-llm/src/{lib.rs,mock.rs}`, `crates/xiaoguai-agent/src/react.rs` all confirmed read. |
| 2 | Every step proposes a runnable verification | **PASS** — each step ends with a `VC:` line (`cargo build` / `cargo test` / `cargo clippy` / `gh pr create`). |
| 3 | Each task has a measurable outcome | **PASS** — success is "27 → 33+ tests passing in `xiaoguai-orchestrator`", `cargo clippy -D warnings` clean, one new file + one re-export. |
| 4 | Out-of-scope is honoured | **PASS** — §5 calls out WorkerAgent, CriticAgent, pattern wiring, integration tests, persona-repo coupling, multi-round memory all as deferred. |
| 5 | Risks have mitigations | **PASS** — §7 lists 5 risks with concrete mitigation each (sentinel substitution, retry coaching, etc.). |
| 6 | Time estimates are sane | **PASS** — roadmap allots 2 dev-days; plan splits into 8 steps each ≤ 2 hours → ~ 8 hours code + 4 hours review = ~ 1.5 days. Conservative. |

### Soft spots flagged

1. **Not wrapping `ReactAgent`.** Deviation A documented above. Strong
   case for going one level down (zero tool calls) but the brief
   explicitly says wrap. Reviewer call.
2. **`MODEL_PLACEHOLDER` constant.** Will be replaced in S9-5; for now
   it's a doc-only string. Flagged for the next sub-agent to remember
   to plumb a real model name through `PlannerAgent::new`.
3. **`CapturingBackend` test fixture lives inside `planner_agent.rs`.**
   If S9-3 / S9-4 want the same fixture they'll duplicate it. If the
   reviewer wants it lifted to a shared test helper, the natural home
   is `crates/xiaoguai-llm/src/mock.rs` as a `MockBackend::capturing()`
   constructor. Out of scope for this PR but flagged for follow-up.
4. **Retry-on-validation behaviour.** Currently set to "no retry on
   validation errors" to match the brief's literal text. The original
   draft retried on validation too (Deviation C). Reverting was a
   deliberate choice; we accept the slightly worse user experience in
   exchange for a smaller blast radius and a sharper error signal.
5. **No streaming events.** The Planner blocks until it has a `Plan`
   or an error. For long planning calls (~ 5 s for a 7-task plan on
   Sonnet) the upstream pattern runner sees no "thinking" event.
   Acceptable for v1.6 — pattern-level event surface is S9-5's
   responsibility.
