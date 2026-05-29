//! `PlannerAgent` — sprint-9 S9-2 / DEC-021 / `lld-orchestrator.md` §4.4.
//!
//! Thin wrapper around `LlmBackend` that produces a single `Plan` per
//! call. The Planner takes **zero tool calls** — it observes the goal +
//! memory snapshot and emits a JSON plan, full stop. That's why we
//! don't wrap `xiaoguai_agent::ReactAgent` directly: `ReactAgent`'s
//! contract is "loop until done with tools" and the Planner is
//! "one-shot, no tools". We use the same primitive `ReactAgent` uses
//! under the hood — `LlmBackend::chat_stream` — without the loop layer
//! that doesn't apply here. See `docs/plans/2026-05-31-sprint9-s9-2-planner-agent.md`
//! Deviation A for the discussion.
//!
//! Retry semantics: on parse failure we retry **once** with the parse
//! error injected into the next system prompt. On validation failure
//! (e.g. empty `tasks` array) we return `PlannerError::PlanInvalid`
//! immediately — the LLM produced parseable output but the rubric is
//! broken, and the orchestrator may want to escalate rather than burn
//! another planner LLM call. Two malformed JSON attempts return
//! `PlannerError::MalformedJson { attempts: 2, last_error }`.
//!
//! Wire shape (what the LLM emits):
//!
//! ```json
//! {
//!   "goal": "string",
//!   "tasks": [
//!     {
//!       "description": "string",
//!       "acceptance_criteria": {
//!         "rubric": "string",
//!         "required_citation_pattern": null,
//!         "min_confidence": null
//!       },
//!       "depends_on_index": null
//!     }
//!   ]
//! }
//! ```
//!
//! `TaskId`s are NOT supplied by the LLM — the orchestrator assigns
//! fresh `TaskId::new()` ids per parse. The LLM expresses dependencies
//! via `depends_on_index` (position in its own `tasks` array); we
//! resolve those to real `TaskId`s after id assignment.

use std::sync::Arc;

use chrono::Utc;
use futures::StreamExt;
use serde::Deserialize;
use thiserror::Error;
use xiaoguai_llm::{ChatRequest, LlmBackend, LlmError, Message};

use crate::triangle::memory_view::MemorySnapshot;
use crate::triangle::plan::{AcceptanceCriteria, Plan, PlanValidationError, Task, TaskId};

/// Sentinel used inside `RETRY_COACHING` for substitution. Deliberately
/// chosen to be unlikely to appear verbatim in a persona prompt —
/// using a plain `{ERR}` would collide if the persona prompt happens
/// to contain the literal three characters `{ERR}`.
const RETRY_ERR_MARKER: &str = "{__PLANNER_LAST_ERR__}";

const RETRY_COACHING: &str = concat!(
    "\n\n",
    "Your previous attempt failed: {__PLANNER_LAST_ERR__}\n",
    "Return ONLY a JSON object matching the schema above — no prose, ",
    "no commentary, no markdown code fences.",
);

const PLAN_SCHEMA_PROMPT: &str = r#"

You MUST respond with a single JSON object matching this schema:

{
  "goal": "string — restate the user's goal in one sentence",
  "tasks": [
    {
      "description": "string — what this task accomplishes",
      "acceptance_criteria": {
        "rubric": "string — what the critic checks for",
        "required_citation_pattern": null,
        "min_confidence": null
      },
      "depends_on_index": null
    }
  ]
}

Rules:
- Emit `tasks` as a non-empty array. Each task is independently
  executable by a Worker agent.
- `depends_on_index` is `null` for independent tasks, or the zero-based
  index of an earlier task in the same array.
- `required_citation_pattern` and `min_confidence` may be `null` if
  there is no constraint.
- Do NOT include task ids — the orchestrator assigns them.
- Do NOT wrap the JSON in markdown fences. Plain JSON only.
"#;

/// Placeholder model field. `MockBackend` ignores it; production
/// callers will provide a real model through the persona config when
/// the Triangle pattern wiring lands in S9-5. Keeping a constant
/// here (rather than a `model: String` field on `PlannerAgent`)
/// avoids API churn — see plan §6 Deviation B.
const MODEL_PLACEHOLDER: &str = "planner";

/// One-shot planner. Stateless across `.plan()` calls; the
/// orchestrator owns the round counter and passes a fresh
/// `MemorySnapshot` per round.
pub struct PlannerAgent {
    inner: Arc<dyn LlmBackend>,
    persona_prompt: String,
    max_retry: u32,
}

impl PlannerAgent {
    /// `max_retry` defaults to `1` — one fresh attempt + one retry on
    /// JSON parse failure = 2 LLM calls worst case.
    #[must_use]
    pub fn new(inner: Arc<dyn LlmBackend>, persona_prompt: String) -> Self {
        Self {
            inner,
            persona_prompt,
            max_retry: 1,
        }
    }

    /// Customise the retry budget. `0` disables retry entirely.
    #[must_use]
    pub fn with_max_retry(mut self, max_retry: u32) -> Self {
        self.max_retry = max_retry;
        self
    }

    /// Produce a `Plan` for `goal` against the shared `memory` snapshot.
    ///
    /// # Errors
    /// - `LlmError` — the backend itself failed (network / quota / …).
    /// - `MalformedJson { attempts, last_error }` — all attempts
    ///   returned text that failed `serde_json::from_str`.
    /// - `PlanInvalid(PlanValidationError)` — the JSON parsed but
    ///   `Plan::validate()` rejected it.
    /// - `BudgetExhausted` — reserved for the pattern runner to inject;
    ///   the agent itself does not raise this today.
    pub async fn plan(&self, goal: &str, memory: &MemorySnapshot) -> Result<Plan, PlannerError> {
        let max_attempts = self.max_retry + 1;
        let mut last_error: Option<String> = None;

        for _attempt in 1..=max_attempts {
            let system = build_system_prompt(&self.persona_prompt, memory, last_error.as_deref());
            let messages = vec![
                Message::system(system),
                Message::user(format!("Goal: {goal}")),
            ];
            let request = ChatRequest::new(MODEL_PLACEHOLDER, messages);

            let text = collect_text(&self.inner, request).await?;

            match parse_and_build(&text, memory, goal) {
                Ok(plan) => return Ok(plan),
                Err(ParseOrValidate::Parse(e)) => {
                    last_error = Some(format!("JSON parse error: {e}"));
                }
                Err(ParseOrValidate::Validate(v)) => {
                    // Validation errors are returned immediately — the
                    // LLM produced valid JSON but a broken rubric;
                    // retrying with "your tasks list was empty" rarely
                    // helps and burns budget. The brief specifies
                    // retry on "JSON parse fail" only.
                    return Err(PlannerError::PlanInvalid(v));
                }
            }
        }

        Err(PlannerError::MalformedJson {
            attempts: max_attempts,
            last_error: last_error.unwrap_or_default(),
        })
    }
}

#[derive(Debug, Error)]
pub enum PlannerError {
    #[error("llm backend error: {0}")]
    LlmError(#[from] LlmError),
    #[error("planner produced malformed JSON after {attempts} attempt(s): {last_error}")]
    MalformedJson { attempts: u32, last_error: String },
    #[error("plan validation failed: {0}")]
    PlanInvalid(#[from] PlanValidationError),
    #[error("planner budget exhausted before a valid plan was produced")]
    BudgetExhausted,
}

// --- prompt rendering ---------------------------------------------------

fn render_memory(snapshot: &MemorySnapshot) -> String {
    if snapshot.facts.is_empty() {
        return format!(
            "Memory snapshot (round {}, captured {}): (empty)\n",
            snapshot.round,
            snapshot.captured_at.to_rfc3339()
        );
    }
    let mut buf = format!(
        "Memory snapshot (round {}, captured {}):\n",
        snapshot.round,
        snapshot.captured_at.to_rfc3339()
    );
    for fact in &snapshot.facts {
        buf.push_str("- ");
        buf.push_str(&fact.key);
        buf.push_str(": ");
        buf.push_str(&fact.value);
        buf.push('\n');
    }
    buf
}

fn build_system_prompt(
    persona_prompt: &str,
    snapshot: &MemorySnapshot,
    last_error: Option<&str>,
) -> String {
    let mut out = String::with_capacity(persona_prompt.len() + 512);
    out.push_str(persona_prompt);
    out.push_str("\n\n");
    out.push_str(&render_memory(snapshot));
    out.push_str(PLAN_SCHEMA_PROMPT);
    if let Some(err) = last_error {
        out.push_str(&RETRY_COACHING.replace(RETRY_ERR_MARKER, err));
    }
    out
}

// --- LLM stream collection ---------------------------------------------

async fn collect_text(
    backend: &Arc<dyn LlmBackend>,
    request: ChatRequest,
) -> Result<String, PlannerError> {
    let mut stream = backend.chat_stream(request).await?;
    let mut text = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if !chunk.delta.is_empty() {
            text.push_str(&chunk.delta);
        }
    }
    Ok(text)
}

// --- wire types + parse/validate ---------------------------------------

#[derive(Deserialize)]
struct PlanWire {
    goal: String,
    tasks: Vec<TaskWire>,
}

#[derive(Deserialize)]
struct TaskWire {
    description: String,
    acceptance_criteria: AcceptanceCriteriaWire,
    #[serde(default)]
    depends_on_index: Option<usize>,
}

#[derive(Deserialize)]
struct AcceptanceCriteriaWire {
    rubric: String,
    #[serde(default)]
    required_citation_pattern: Option<String>,
    #[serde(default)]
    min_confidence: Option<f32>,
}

/// Disambiguates retry-eligible (Parse) from retry-ineligible
/// (Validate) failures. Kept private; the public API exposes the two
/// outcomes as distinct `PlannerError` variants.
enum ParseOrValidate {
    Parse(serde_json::Error),
    Validate(PlanValidationError),
}

fn parse_and_build(
    text: &str,
    memory: &MemorySnapshot,
    fallback_goal: &str,
) -> Result<Plan, ParseOrValidate> {
    let wire: PlanWire = serde_json::from_str(text.trim()).map_err(ParseOrValidate::Parse)?;

    // Assign fresh ids per task. We allocate ids first so that
    // `depends_on_index` lookups can resolve to a real `TaskId` in
    // the same pass.
    let assigned_ids: Vec<TaskId> = (0..wire.tasks.len()).map(|_| TaskId::new()).collect();

    let mut tasks = Vec::with_capacity(wire.tasks.len());
    for (i, t) in wire.tasks.into_iter().enumerate() {
        let depends_on = match t.depends_on_index {
            None => None,
            Some(position) if position < assigned_ids.len() => Some(assigned_ids[position]),
            Some(_) => {
                // Out-of-range index → treat as a parse error so the
                // retry loop gets a shot. Synthesise a serde error by
                // attempting to parse a known-bad JSON string.
                return Err(ParseOrValidate::Parse(
                    serde_json::from_str::<()>("__depends_on_index_out_of_range__").unwrap_err(),
                ));
            }
        };
        tasks.push(Task {
            id: assigned_ids[i],
            description: t.description,
            acceptance_criteria: AcceptanceCriteria {
                rubric: t.acceptance_criteria.rubric,
                required_citation_pattern: t.acceptance_criteria.required_citation_pattern,
                min_confidence: t.acceptance_criteria.min_confidence,
            },
            depends_on,
        });
    }

    // Prefer the LLM-restated goal if non-empty; fall back to the
    // caller's goal string to keep `Plan::goal` non-empty when the LLM
    // omits it (validate() would reject empty-goal otherwise).
    let goal = if wire.goal.trim().is_empty() {
        fallback_goal.to_string()
    } else {
        wire.goal
    };

    let plan = Plan {
        round: memory.round,
        goal,
        tasks,
        created_at: Utc::now(),
    };

    plan.validate().map_err(ParseOrValidate::Validate)?;
    Ok(plan)
}

// =======================================================================
// Tests
// =======================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    use async_trait::async_trait;
    use chrono::Utc;
    use futures::stream;
    use xiaoguai_llm::{backend::ChatStream, types::ChatChunk, FinishReason, LlmError};

    use crate::triangle::memory_view::MemoryFact;

    // ---- test fixture: capturing backend ------------------------------

    /// Backend that records the last `ChatRequest` it saw and replays a
    /// scripted sequence of text responses. The script is drained
    /// front-to-back; once exhausted, the final entry replays. Mirrors
    /// `MockBackend::with_script` semantics but exposes the captured
    /// requests for prompt-content assertions (tests 5 + 6).
    #[derive(Default)]
    struct CapturingBackend {
        captured: Arc<Mutex<Vec<ChatRequest>>>,
        scripted: Arc<Mutex<Vec<String>>>,
    }

    impl CapturingBackend {
        fn new(scripted: Vec<&str>) -> Arc<Self> {
            Arc::new(Self {
                captured: Arc::new(Mutex::new(Vec::new())),
                scripted: Arc::new(Mutex::new(scripted.into_iter().map(String::from).collect())),
            })
        }

        fn last_request(&self) -> ChatRequest {
            self.captured
                .lock()
                .unwrap()
                .last()
                .cloned()
                .expect("no request captured yet")
        }

        fn request_count(&self) -> usize {
            self.captured.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl LlmBackend for CapturingBackend {
        async fn chat_stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
            self.captured.lock().unwrap().push(req);
            let text = {
                let mut guard = self.scripted.lock().unwrap();
                if guard.len() > 1 {
                    guard.remove(0)
                } else {
                    guard[0].clone()
                }
            };
            let chunks = vec![
                Ok(ChatChunk {
                    delta: text,
                    ..Default::default()
                }),
                Ok(ChatChunk {
                    delta: String::new(),
                    tool_calls: Vec::new(),
                    finish_reason: Some(FinishReason::Stop),
                    done: true,
                    reasoning_delta: None,
                }),
            ];
            Ok(Box::pin(stream::iter(chunks)))
        }

        fn name(&self) -> &'static str {
            "capturing"
        }
    }

    fn empty_snapshot(round: u32) -> MemorySnapshot {
        MemorySnapshot {
            round,
            facts: Vec::new(),
            captured_at: Utc::now(),
        }
    }

    const VALID_PLAN_JSON: &str = r#"{
        "goal": "summarise the Q3 release",
        "tasks": [
            {
                "description": "collect Q3 PR titles",
                "acceptance_criteria": {
                    "rubric": "non-empty list of PR titles",
                    "required_citation_pattern": null,
                    "min_confidence": null
                },
                "depends_on_index": null
            },
            {
                "description": "draft the summary paragraph",
                "acceptance_criteria": {
                    "rubric": "<= 3 paragraphs, mentions release date",
                    "required_citation_pattern": "PR #",
                    "min_confidence": 0.7
                },
                "depends_on_index": 0
            }
        ]
    }"#;

    // ---- 1. happy path -----------------------------------------------

    #[tokio::test]
    async fn happy_path_produces_plan_with_tasks() {
        let backend = CapturingBackend::new(vec![VALID_PLAN_JSON]);
        let planner = PlannerAgent::new(
            backend.clone() as Arc<dyn LlmBackend>,
            "You are the Planner.".into(),
        );

        let snap = empty_snapshot(0);
        let plan = planner
            .plan("summarise the Q3 release", &snap)
            .await
            .expect("plan should parse");

        assert_eq!(plan.round, 0);
        assert_eq!(plan.goal, "summarise the Q3 release");
        assert_eq!(plan.tasks.len(), 2);
        assert!(plan.tasks[0].depends_on.is_none());
        assert_eq!(plan.tasks[1].depends_on, Some(plan.tasks[0].id));
        // IDs are unique (assigned by the orchestrator, not the LLM):
        assert_ne!(plan.tasks[0].id, plan.tasks[1].id);
        // Acceptance criteria fields plumbed correctly:
        assert_eq!(
            plan.tasks[1].acceptance_criteria.required_citation_pattern,
            Some("PR #".to_string())
        );
        assert_eq!(plan.tasks[1].acceptance_criteria.min_confidence, Some(0.7));
        // Single LLM call (no retry needed):
        assert_eq!(backend.request_count(), 1);
    }

    // ---- 2. malformed then valid: retry succeeds ---------------------

    #[tokio::test]
    async fn malformed_then_valid_succeeds_on_retry() {
        let backend = CapturingBackend::new(vec!["this is not JSON, sorry", VALID_PLAN_JSON]);
        let planner = PlannerAgent::new(
            backend.clone() as Arc<dyn LlmBackend>,
            "You are the Planner.".into(),
        );

        let snap = empty_snapshot(2);
        let plan = planner
            .plan("summarise the Q3 release", &snap)
            .await
            .expect("retry should produce a valid plan");

        assert_eq!(plan.tasks.len(), 2);
        assert_eq!(plan.round, 2);
        assert_eq!(backend.request_count(), 2);

        // The second (retry) prompt should carry the parse error.
        let second_system = backend.captured.lock().unwrap()[1]
            .messages
            .iter()
            .find(|m| matches!(m.role, xiaoguai_llm::Role::System))
            .map(|m| m.content.clone())
            .expect("system message");
        assert!(
            second_system.contains("Your previous attempt failed"),
            "retry prompt should include error coaching, got: {second_system}"
        );
        assert!(
            second_system.contains("JSON parse error"),
            "retry prompt should mention JSON parse error, got: {second_system}"
        );
    }

    // ---- 3. two malformed attempts -----------------------------------

    #[tokio::test]
    async fn two_malformed_attempts_returns_malformed_json() {
        let backend = CapturingBackend::new(vec!["still not JSON", "also not JSON {{"]);
        let planner = PlannerAgent::new(
            backend.clone() as Arc<dyn LlmBackend>,
            "You are the Planner.".into(),
        );

        let snap = empty_snapshot(0);
        let err = planner
            .plan("g", &snap)
            .await
            .expect_err("should fail after retry");

        match err {
            PlannerError::MalformedJson {
                attempts,
                last_error,
            } => {
                assert_eq!(attempts, 2);
                assert!(
                    last_error.contains("JSON parse error"),
                    "expected last_error to mention parse, got: {last_error}"
                );
            }
            other => panic!("expected MalformedJson, got {other:?}"),
        }
        assert_eq!(backend.request_count(), 2);
    }

    // ---- 4. validation failure (empty tasks) -------------------------

    #[tokio::test]
    async fn empty_tasks_returns_plan_invalid() {
        let json = r#"{ "goal": "g", "tasks": [] }"#;
        let backend = CapturingBackend::new(vec![json]);
        let planner = PlannerAgent::new(
            backend.clone() as Arc<dyn LlmBackend>,
            "You are the Planner.".into(),
        );

        let snap = empty_snapshot(0);
        let err = planner
            .plan("g", &snap)
            .await
            .expect_err("empty tasks should reject");

        match err {
            PlannerError::PlanInvalid(PlanValidationError::EmptyTasks) => {}
            other => panic!("expected PlanInvalid(EmptyTasks), got {other:?}"),
        }
        // Single attempt — validation errors do NOT retry per the brief.
        assert_eq!(backend.request_count(), 1);
    }

    // ---- 5. persona prompt verbatim in request -----------------------

    #[tokio::test]
    async fn persona_prompt_appears_in_request() {
        let persona = "You are Athena, the master strategist. \
                       Decompose goals into 3-5 atomic steps.";
        let backend = CapturingBackend::new(vec![VALID_PLAN_JSON]);
        let planner = PlannerAgent::new(backend.clone() as Arc<dyn LlmBackend>, persona.into());

        let snap = empty_snapshot(0);
        let _ = planner.plan("g", &snap).await.expect("plan");

        let req = backend.last_request();
        let system = req
            .messages
            .iter()
            .find(|m| matches!(m.role, xiaoguai_llm::Role::System))
            .map(|m| m.content.clone())
            .expect("system message present");
        assert!(
            system.starts_with(persona),
            "persona prompt should be the prefix of the system message"
        );
    }

    // ---- 6. memory facts rendered into prompt -----------------------

    #[tokio::test]
    async fn memory_facts_appear_in_prompt() {
        let backend = CapturingBackend::new(vec![VALID_PLAN_JSON]);
        let planner = PlannerAgent::new(
            backend.clone() as Arc<dyn LlmBackend>,
            "You are the Planner.".into(),
        );

        let snap = MemorySnapshot {
            round: 7,
            facts: vec![
                MemoryFact {
                    key: "region".into(),
                    value: "us-east-1".into(),
                },
                MemoryFact {
                    key: "model".into(),
                    value: "claude-sonnet-4-6".into(),
                },
            ],
            captured_at: Utc::now(),
        };
        let _ = planner.plan("g", &snap).await.expect("plan");

        let req = backend.last_request();
        let system = req
            .messages
            .iter()
            .find(|m| matches!(m.role, xiaoguai_llm::Role::System))
            .map(|m| m.content.clone())
            .expect("system message present");

        assert!(
            system.contains("region: us-east-1"),
            "first fact missing: {system}"
        );
        assert!(
            system.contains("model: claude-sonnet-4-6"),
            "second fact missing: {system}"
        );
        assert!(system.contains("round 7"), "round number missing: {system}");
    }

    // ---- bonus: user goal is included --------------------------------

    #[tokio::test]
    async fn goal_appears_in_user_message() {
        let backend = CapturingBackend::new(vec![VALID_PLAN_JSON]);
        let planner = PlannerAgent::new(
            backend.clone() as Arc<dyn LlmBackend>,
            "You are the Planner.".into(),
        );

        let snap = empty_snapshot(0);
        let _ = planner
            .plan("draft a runbook for X", &snap)
            .await
            .expect("plan");

        let req = backend.last_request();
        let user = req
            .messages
            .iter()
            .find(|m| matches!(m.role, xiaoguai_llm::Role::User))
            .map(|m| m.content.clone())
            .expect("user message present");
        assert!(user.contains("draft a runbook for X"));
    }
}
