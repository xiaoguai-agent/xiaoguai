//! `WorkerAgent` — DEC-021 §4.4 Worker step.
//!
//! Wraps an inner `xiaoguai_agent::ReactAgent` to execute one `Task` as
//! a full ReAct loop. Per the §4.5 quarantine invariant, each call to
//! [`WorkerAgent::execute`] spawns a **fresh** `ReactAgent` and writes
//! only to the per-task `Scratchpad` it's handed. No state leaks
//! across tasks; no Worker reads another Worker's notes.
//!
//! Cost-tracking caveat: `ReactAgent` does not yet surface
//! per-iteration backend token usage. We approximate cost by
//! accumulating `TextDelta` events between iteration boundaries and
//! running them through [`xiaoguai_llm::estimate_tokens`]. This
//! under-counts reasoning tokens but is monotone and deterministic,
//! which is enough to drive the §4.4 budget gate. Production wiring
//! will replace this with backend `usage` reports in a later sprint.
//!
//! Test affordance: the inner `ReactAgent` is constructed with an
//! **empty** `Toolbox`. Scripted backends that request tool calls
//! will see "tool {name:?} not in toolbox" tool-result messages and
//! the ReAct loop continues — that's the mechanism the multi-iteration
//! tests use to drive turn count without standing up an MCP stub.
//! S9-5 wires a real toolbox in `patterns/triangle.rs`.

use std::sync::Arc;

use once_cell::sync::Lazy;
use regex::Regex;
use thiserror::Error;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;
use xiaoguai_agent::{AgentConfig, AgentEvent, AgentOutcome, ReactAgent, StopReason, Toolbox};
use xiaoguai_llm::{estimate_tokens, LlmBackend, LlmError, Message, Role};

use super::memory_view::MemorySnapshot;
use super::plan::{Task, TaskId};
use super::scratchpad::Scratchpad;

/// Default model label passed to the inner `AgentConfig`. The
/// `MockBackend` ignores this; production wiring substitutes the
/// persona's preferred model in S9-5.
const DEFAULT_MODEL: &str = "worker";

/// Default cap on inner ReAct iterations — mirrors `AgentConfig::new`.
/// Tests pin this to a lower value via [`WorkerAgent::with_max_iterations`].
const DEFAULT_MAX_ITERATIONS: u32 = 8;

/// Default confidence when the Worker's final message omits a
/// `"confidence": <float>` marker. 0.5 is the neutral midpoint — the
/// Critic should treat absent-self-report as "no signal", not as
/// "high confidence".
const DEFAULT_CONFIDENCE: f32 = 0.5;

/// Wrapper around `ReactAgent` for the triangle's Worker role.
/// Stateless across calls — holds only configuration.
pub struct WorkerAgent {
    backend: Arc<dyn LlmBackend>,
    persona_prompt: String,
    #[allow(
        dead_code,
        reason = "S9-3 stores the allowlist; S9-5 will filter the real toolbox by it"
    )]
    tool_allowlist: Vec<String>,
    model: String,
    max_iterations: u32,
}

impl WorkerAgent {
    /// Construct a Worker. `persona_prompt` is the system-prompt
    /// preamble that defines the Worker persona; `tool_allowlist`
    /// is the set of tool names the inner ReAct loop is permitted to
    /// invoke (S9-3 stores it; S9-5 wires it through to the toolbox).
    #[must_use]
    pub fn new(
        backend: Arc<dyn LlmBackend>,
        persona_prompt: String,
        tool_allowlist: Vec<String>,
    ) -> Self {
        Self {
            backend,
            persona_prompt,
            tool_allowlist,
            model: DEFAULT_MODEL.to_string(),
            max_iterations: DEFAULT_MAX_ITERATIONS,
        }
    }

    /// Override the inner ReAct max-iteration cap. Used by the
    /// MaxIterations regression test to force a low ceiling.
    #[must_use]
    pub fn with_max_iterations(mut self, n: u32) -> Self {
        self.max_iterations = n;
        self
    }

    /// Override the model label propagated onto the inner `AgentConfig`.
    /// Production callers thread the persona's preferred model here.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Execute one `Task` as a full ReAct loop, writing intermediate
    /// state to `scratchpad`. Returns a [`WorkerResult`] bundling the
    /// artefact + cost. Budget is enforced at iteration boundaries
    /// (cf. DEC-021 §4.5) — checked after each `IterationCompleted`
    /// event; never interrupts mid-LLM-call.
    ///
    /// # Errors
    /// - [`WorkerError::WrongTaskId`] — the scratchpad's `task_id`
    ///   doesn't match `task.id` (defensive against misrouted
    ///   dispatch).
    /// - [`WorkerError::BudgetTooSmall`] — `budget_tokens == 0`.
    /// - [`WorkerError::LlmError`] — the inner ReactAgent's join
    ///   handle returned a backend-level error.
    pub async fn execute(
        &self,
        task: &Task,
        scratchpad: &mut Scratchpad,
        memory: &MemorySnapshot,
        budget_tokens: u64,
    ) -> Result<WorkerResult, WorkerError> {
        // 1. Defensive task-id check. We do NOT actually write a
        //    sentinel entry — the goal is to refuse misrouted
        //    dispatch *before* the inner loop spins up.
        if scratchpad.task_id != task.id {
            return Err(WorkerError::WrongTaskId {
                expected: scratchpad.task_id,
                actual: task.id,
            });
        }

        if budget_tokens == 0 {
            return Err(WorkerError::BudgetTooSmall);
        }

        // 2. Build initial messages.
        //    System: persona + memory facts (read-only round snapshot).
        //    User: task description + acceptance-criteria rubric.
        let system_msg = Message::system(format!(
            "{persona}\n\nMemory facts (round {round}):\n{facts}",
            persona = self.persona_prompt,
            round = memory.round,
            facts = format_memory(memory),
        ));
        let user_msg = Message::user(format!(
            "Task: {desc}\n\nAcceptance criteria: {rubric}",
            desc = task.description,
            rubric = task.acceptance_criteria.rubric,
        ));

        // 3. Drive ReactAgent **one iteration at a time** so we can
        //    enforce the budget gate at exact iteration boundaries
        //    (DEC-021 §4.5: "check before each iteration"). Running
        //    the inner agent in `run_stream` mode and trying to cancel
        //    races against the buffered event channel — at small mock
        //    latencies the inner loop completes before our cancel
        //    arrives. Looping with `max_iterations=1` per call avoids
        //    that race entirely.
        let mut messages = vec![system_msg, user_msg];
        let mut total_iterations: u32 = 0;
        let mut final_stop = StopReason::MaxIterations;
        let mut tool_error: Option<String> = None;
        let mut budget_exhausted = false;

        while total_iterations < self.max_iterations {
            // 3a. Budget gate — runs BEFORE each iteration. The first
            //     iteration always proceeds (budget_tokens > 0 was
            //     checked above).
            if scratchpad.cost_tokens >= budget_tokens {
                budget_exhausted = true;
                final_stop = StopReason::Cancelled;
                break;
            }

            // 3b. One ReAct cycle. We use a fresh CancellationToken
            //     per call so an Error event in the previous iteration
            //     does not leak into this one.
            let inner_cfg = AgentConfig {
                max_iterations: 1,
                history_window: 64,
                temperature: Some(0.2),
                model: self.model.clone(),
                tenant_id: None,
                hotl_gate: None,
                compaction: None,
            };
            let react =
                ReactAgent::new(self.backend.clone(), Toolbox::new(), inner_cfg);
            let cancel = CancellationToken::new();
            let (handle, mut stream) = react.run_stream(messages.clone(), cancel);

            // Drain events for THIS iteration only.
            let mut delta_buf = String::new();
            while let Some(event) = stream.next().await {
                match event {
                    AgentEvent::TextDelta { delta } => delta_buf.push_str(&delta),
                    AgentEvent::Error { message } => {
                        tool_error = Some(message);
                    }
                    AgentEvent::IterationCompleted { .. }
                    | AgentEvent::Done { .. }
                    | AgentEvent::ToolCallStarted { .. }
                    | AgentEvent::ToolCallFinished { .. } => {}
                }
            }

            let outcome = handle.await.map_err(|e| {
                WorkerError::LlmError(LlmError::Provider(format!(
                    "ReactAgent join error: {e}"
                )))
            })??;

            // 3c. Persist messages for the next pass + count.
            messages = outcome.messages;
            // The inner loop either completed (no tool calls) or hit
            // MaxIterations=1 (had tool calls + tool results pushed).
            // Either way that's one ReAct cycle from our perspective.
            total_iterations += 1;

            // 3d. Tokens-used estimate for this iteration. We
            //     accumulate text deltas (best-effort heuristic; see
            //     module doc-comment).
            let tokens_used = estimate_tokens(&delta_buf).max(1) as u32;
            let summary = if delta_buf.trim().is_empty() {
                format!(
                    "iteration {idx}: (tool calls only, no text)",
                    idx = total_iterations - 1,
                )
            } else {
                format!(
                    "iteration {idx}: {body}",
                    idx = total_iterations - 1,
                    body = delta_buf.trim(),
                )
            };
            scratchpad
                .append(task.id, summary, Some(tokens_used))
                .expect("scratchpad task_id verified above; non-empty content");

            // 3e. Tool error from the inner loop short-circuits.
            if tool_error.is_some() {
                final_stop = StopReason::Cancelled;
                break;
            }

            // 3f. Inner outcome tells us whether the loop is done.
            //     `Completed` = model emitted text without tool calls.
            //     `MaxIterations` = it had more work to do (we run
            //     max=1 each pass, so this is the "continue" signal).
            match outcome.stop_reason {
                StopReason::Completed => {
                    final_stop = StopReason::Completed;
                    break;
                }
                StopReason::MaxIterations => {
                    // Continue — fold tool results back in via
                    // `messages` and loop.
                }
                StopReason::Cancelled => {
                    final_stop = StopReason::Cancelled;
                    break;
                }
            }
        }

        // If we exited the loop without setting a terminal stop, it's
        // because we ran the outer cap exactly. Mark MaxIterations.
        if total_iterations >= self.max_iterations
            && !budget_exhausted
            && tool_error.is_none()
            && final_stop != StopReason::Completed
        {
            final_stop = StopReason::MaxIterations;
        }

        let outcome = AgentOutcome {
            stop_reason: final_stop,
            messages,
            iterations: total_iterations,
        };

        // 4. Translate stop reason.
        let stop_reason = classify_stop(&outcome, budget_exhausted, tool_error.clone());

        // 7. Extract artefact (None if we didn't complete).
        let artefact = if matches!(stop_reason, WorkerStopReason::Completed) {
            find_final_assistant_text(&outcome.messages)
        } else {
            None
        };

        // 8/9. Confidence + citations from whatever artefact text we
        //      have. If artefact is None we still try to extract from
        //      the LAST assistant message (debugging signal) but only
        //      if it carries content.
        let scan_target = artefact
            .clone()
            .or_else(|| find_final_assistant_text(&outcome.messages))
            .unwrap_or_default();
        let confidence = parse_confidence(&scan_target);
        let citations = extract_citations(&scan_target);

        Ok(WorkerResult {
            task_id: task.id,
            artefact,
            citations,
            confidence,
            cost_tokens: scratchpad.cost_tokens,
            iterations: outcome.iterations,
            stop_reason,
        })
    }
}

/// Output of a single Worker execution. Aggregated by the
/// orchestrator into the next planning round's `MemorySnapshot` only
/// after the Critic approves it (DEC-021 §4.5).
#[derive(Debug, Clone)]
pub struct WorkerResult {
    pub task_id: TaskId,
    /// Final assistant text. `None` if the Worker gave up
    /// (BudgetExhausted / MaxIterations / ToolError).
    pub artefact: Option<String>,
    /// Best-effort: URLs + bracketed numeric refs found in the artefact.
    pub citations: Vec<String>,
    /// 0.0..=1.0. Self-reported via `"confidence": <float>` marker in
    /// the final message; defaults to 0.5 when absent.
    pub confidence: f32,
    /// Matches `scratchpad.cost_tokens` at return time — single
    /// source of truth for the orchestrator's budget bookkeeping.
    pub cost_tokens: u64,
    /// Number of ReAct cycles the inner loop executed.
    pub iterations: u32,
    pub stop_reason: WorkerStopReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerStopReason {
    Completed,
    MaxIterations,
    BudgetExhausted,
    ToolError(String),
}

#[derive(Debug, Error)]
pub enum WorkerError {
    #[error("inner LLM backend error: {0}")]
    LlmError(#[from] LlmError),
    #[error(
        "scratchpad quarantine violation: expected task {expected}, got task {actual}"
    )]
    WrongTaskId { expected: TaskId, actual: TaskId },
    #[error("budget_tokens must be > 0")]
    BudgetTooSmall,
}

// `xiaoguai_agent::AgentError` -> `WorkerError` so `handle.await??` works.
impl From<xiaoguai_agent::AgentError> for WorkerError {
    fn from(e: xiaoguai_agent::AgentError) -> Self {
        match e {
            xiaoguai_agent::AgentError::Llm(le) => Self::LlmError(le),
            xiaoguai_agent::AgentError::Other(msg) => {
                Self::LlmError(LlmError::Provider(msg))
            }
        }
    }
}

// --- helpers ---------------------------------------------------------

fn format_memory(snap: &MemorySnapshot) -> String {
    if snap.facts.is_empty() {
        return "(none)".to_string();
    }
    snap.facts
        .iter()
        .map(|f| format!("- {}: {}", f.key, f.value))
        .collect::<Vec<_>>()
        .join("\n")
}

fn find_final_assistant_text(msgs: &[Message]) -> Option<String> {
    msgs.iter()
        .rev()
        .find(|m| m.role == Role::Assistant && !m.content.trim().is_empty())
        .map(|m| m.content.clone())
}

fn classify_stop(
    outcome: &AgentOutcome,
    budget_exhausted: bool,
    tool_error: Option<String>,
) -> WorkerStopReason {
    match outcome.stop_reason {
        StopReason::Completed => WorkerStopReason::Completed,
        StopReason::MaxIterations => WorkerStopReason::MaxIterations,
        StopReason::Cancelled => {
            if budget_exhausted {
                WorkerStopReason::BudgetExhausted
            } else if let Some(msg) = tool_error {
                WorkerStopReason::ToolError(msg)
            } else {
                // Defensive — external cancellation we didn't request.
                // Treat as budget-exhausted so the orchestrator at
                // least sees a cost-related signal rather than a
                // bare "cancelled".
                WorkerStopReason::BudgetExhausted
            }
        }
    }
}

/// Confidence regex — match the LAST `"confidence": <float>` in the
/// text. The Worker persona convention puts the self-report at the
/// end of the final message; taking the last match avoids false hits
/// on cited JSON earlier in the text.
static CONFIDENCE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#""confidence"\s*:\s*(-?[0-9]+(?:\.[0-9]+)?)"#).unwrap()
});

fn parse_confidence(text: &str) -> f32 {
    let Some(last) = CONFIDENCE_RE.captures_iter(text).last() else {
        return DEFAULT_CONFIDENCE;
    };
    let raw = last.get(1).map_or("", |m| m.as_str());
    let parsed: f32 = raw.parse().unwrap_or(DEFAULT_CONFIDENCE);
    parsed.clamp(0.0, 1.0)
}

/// URL regex — RFC 3986-lite, stops at whitespace and closing parens
/// (so markdown `(https://x.com)` doesn't capture the trailing `)`).
static URL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"https?://[^\s)\]]+").unwrap());
/// Bracketed-number citation regex — `[1]`, `[42]`, etc.
static BRACKET_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[\d+\]").unwrap());

/// Trim sentence-final punctuation from a captured URL. Without this,
/// regex hits like `https://ref.org.` (with the trailing period)
/// surface as the citation — but readers expect `https://ref.org`.
fn strip_url_trailing_punct(raw: &str) -> &str {
    raw.trim_end_matches(['.', ',', ';', ':', '!', '?', '"', '\''])
}

fn extract_citations(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    // URLs first, in document order.
    for m in URL_RE.find_iter(text) {
        let s = strip_url_trailing_punct(m.as_str()).to_string();
        if !out.contains(&s) {
            out.push(s);
        }
    }
    for m in BRACKET_RE.find_iter(text) {
        let s = m.as_str().to_string();
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triangle::memory_view::{InMemoryMemoryView, MemoryView};
    use crate::triangle::plan::AcceptanceCriteria;
    use xiaoguai_llm::mock::{MockBackend, ScriptStep};
    use xiaoguai_llm::ToolCallSpec;

    fn ac(rubric: &str) -> AcceptanceCriteria {
        AcceptanceCriteria {
            rubric: rubric.to_string(),
            required_citation_pattern: None,
            min_confidence: None,
        }
    }

    fn fresh_task(desc: &str) -> Task {
        Task {
            id: TaskId::new(),
            description: desc.to_string(),
            acceptance_criteria: ac("non-empty answer"),
            depends_on: None,
        }
    }

    async fn snap() -> MemorySnapshot {
        InMemoryMemoryView::new().snapshot(0).await
    }

    fn tc(id: &str, name: &str) -> ToolCallSpec {
        ToolCallSpec {
            id: id.to_string(),
            name: name.to_string(),
            arguments_json: "{}".to_string(),
        }
    }

    #[tokio::test]
    async fn happy_completion_one_iteration() {
        let backend = Arc::new(MockBackend::with_response("the answer"));
        let agent = WorkerAgent::new(backend, "persona".into(), vec![]);
        let task = fresh_task("compute X");
        let mut sp = Scratchpad::new(task.id);
        let memory = snap().await;

        let res = agent.execute(&task, &mut sp, &memory, 10_000).await.unwrap();

        assert_eq!(res.task_id, task.id);
        assert_eq!(res.artefact.as_deref(), Some("the answer"));
        assert_eq!(res.stop_reason, WorkerStopReason::Completed);
        assert_eq!(res.iterations, 1);
        assert!(res.cost_tokens > 0, "cost should accumulate at least 1 token");
        assert_eq!(sp.entries().len(), 1);
        assert_eq!(sp.cost_tokens, res.cost_tokens);
    }

    #[tokio::test]
    async fn multi_iteration_three_tool_calls_then_answer() {
        // 3 tool-call turns + final text => 4 ReAct iterations.
        let script = vec![
            ScriptStep::tool_calls(vec![tc("1", "missing_tool")]),
            ScriptStep::tool_calls(vec![tc("2", "missing_tool")]),
            ScriptStep::tool_calls(vec![tc("3", "missing_tool")]),
            ScriptStep::text("done"),
        ];
        let backend = Arc::new(MockBackend::with_script(script));
        let agent = WorkerAgent::new(backend, "persona".into(), vec![]);
        let task = fresh_task("multi-step");
        let mut sp = Scratchpad::new(task.id);
        let memory = snap().await;

        let res = agent.execute(&task, &mut sp, &memory, 1_000_000).await.unwrap();

        assert_eq!(res.stop_reason, WorkerStopReason::Completed);
        assert_eq!(res.iterations, 4);
        assert_eq!(sp.entries().len(), 4);
        assert_eq!(res.artefact.as_deref(), Some("done"));
    }

    #[tokio::test]
    async fn budget_exhausted_mid_loop() {
        // Each tool-call iteration writes a scratchpad entry of >=1 token.
        // Budget of 2 forces exhaustion after ~2 iterations.
        let script = vec![
            ScriptStep::tool_calls(vec![tc("1", "missing")]),
            ScriptStep::tool_calls(vec![tc("2", "missing")]),
            ScriptStep::tool_calls(vec![tc("3", "missing")]),
            ScriptStep::text("would-be-final"),
        ];
        let backend = Arc::new(MockBackend::with_script(script));
        let agent = WorkerAgent::new(backend, "persona".into(), vec![]);
        let task = fresh_task("expensive");
        let mut sp = Scratchpad::new(task.id);
        let memory = snap().await;

        let res = agent.execute(&task, &mut sp, &memory, 2).await.unwrap();

        assert_eq!(res.stop_reason, WorkerStopReason::BudgetExhausted);
        assert!(res.artefact.is_none(), "no artefact on budget exhaustion");
        assert!(sp.cost_tokens >= 2, "budget gate fires at or above the cap");
        assert!(
            res.iterations < 4,
            "loop should not have reached the final text turn"
        );
    }

    #[tokio::test]
    async fn scratchpad_gets_entry_per_iteration() {
        let script = vec![
            ScriptStep::tool_calls(vec![tc("1", "missing")]),
            ScriptStep::tool_calls(vec![tc("2", "missing")]),
            ScriptStep::text("ok"),
        ];
        let backend = Arc::new(MockBackend::with_script(script));
        let agent = WorkerAgent::new(backend, "persona".into(), vec![]);
        let task = fresh_task("counted");
        let mut sp = Scratchpad::new(task.id);
        let memory = snap().await;

        let res = agent.execute(&task, &mut sp, &memory, 1_000_000).await.unwrap();
        assert_eq!(sp.entries().len() as u32, res.iterations);
    }

    #[tokio::test]
    async fn wrong_task_id_scratchpad_refused() {
        let backend = Arc::new(MockBackend::with_response("would-write"));
        let agent = WorkerAgent::new(backend, "persona".into(), vec![]);
        let task = fresh_task("dispatch me");
        // Scratchpad belongs to a DIFFERENT task — misrouted dispatch.
        let foreign = TaskId::new();
        let mut sp = Scratchpad::new(foreign);
        let memory = snap().await;

        let err = agent
            .execute(&task, &mut sp, &memory, 10_000)
            .await
            .unwrap_err();

        match err {
            WorkerError::WrongTaskId { expected, actual } => {
                assert_eq!(expected, foreign);
                assert_eq!(actual, task.id);
            }
            other => panic!("expected WrongTaskId, got {other:?}"),
        }
        assert!(sp.entries().is_empty(), "no writes on quarantine violation");
        assert_eq!(sp.cost_tokens, 0);
    }

    #[tokio::test]
    async fn confidence_parsed_from_final_message() {
        let final_msg = r#"Here is the answer. "confidence": 0.85"#;
        let backend = Arc::new(MockBackend::with_response(final_msg));
        let agent = WorkerAgent::new(backend, "persona".into(), vec![]);
        let task = fresh_task("conf");
        let mut sp = Scratchpad::new(task.id);
        let memory = snap().await;

        let res = agent.execute(&task, &mut sp, &memory, 10_000).await.unwrap();
        assert!(
            (res.confidence - 0.85).abs() < f32::EPSILON,
            "expected 0.85, got {}",
            res.confidence
        );
    }

    #[tokio::test]
    async fn confidence_defaults_to_half_when_absent() {
        let backend = Arc::new(MockBackend::with_response("no marker here"));
        let agent = WorkerAgent::new(backend, "persona".into(), vec![]);
        let task = fresh_task("default-conf");
        let mut sp = Scratchpad::new(task.id);
        let memory = snap().await;

        let res = agent.execute(&task, &mut sp, &memory, 10_000).await.unwrap();
        assert!(
            (res.confidence - DEFAULT_CONFIDENCE).abs() < f32::EPSILON,
            "expected default {DEFAULT_CONFIDENCE}, got {}",
            res.confidence
        );
    }

    #[tokio::test]
    async fn citations_extracted_from_urls_and_brackets() {
        let final_msg =
            "Per https://example.com/doc and [1], also [42] — see https://ref.org.";
        let backend = Arc::new(MockBackend::with_response(final_msg));
        let agent = WorkerAgent::new(backend, "persona".into(), vec![]);
        let task = fresh_task("cites");
        let mut sp = Scratchpad::new(task.id);
        let memory = snap().await;

        let res = agent.execute(&task, &mut sp, &memory, 10_000).await.unwrap();
        assert!(res.citations.contains(&"https://example.com/doc".to_string()));
        assert!(res.citations.contains(&"https://ref.org".to_string()));
        assert!(res.citations.contains(&"[1]".to_string()));
        assert!(res.citations.contains(&"[42]".to_string()));
    }

    #[tokio::test]
    async fn max_iterations_stop_reason() {
        // Script that always emits tool calls. With max_iterations=2,
        // the inner ReactAgent caps at 2 cycles.
        let script = vec![ScriptStep::tool_calls(vec![tc("loop", "missing")])];
        let backend = Arc::new(MockBackend::with_script(script));
        let agent = WorkerAgent::new(backend, "persona".into(), vec![])
            .with_max_iterations(2);
        let task = fresh_task("infinite");
        let mut sp = Scratchpad::new(task.id);
        let memory = snap().await;

        let res = agent.execute(&task, &mut sp, &memory, 1_000_000).await.unwrap();
        assert_eq!(res.stop_reason, WorkerStopReason::MaxIterations);
        assert!(res.artefact.is_none());
        assert_eq!(res.iterations, 2);
        assert_eq!(sp.entries().len(), 2);
    }

    #[tokio::test]
    async fn budget_too_small_rejected() {
        let backend = Arc::new(MockBackend::with_response("x"));
        let agent = WorkerAgent::new(backend, "persona".into(), vec![]);
        let task = fresh_task("zero-budget");
        let mut sp = Scratchpad::new(task.id);
        let memory = snap().await;

        let err = agent.execute(&task, &mut sp, &memory, 0).await.unwrap_err();
        assert!(matches!(err, WorkerError::BudgetTooSmall));
    }

    // --- helper-function unit tests --------------------------------

    #[test]
    fn parse_confidence_takes_last_match() {
        // Worker convention: self-report at the end.
        let text = r#"some context "confidence": 0.10 then later "confidence": 0.77"#;
        assert!((parse_confidence(text) - 0.77).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_confidence_clamps_out_of_range() {
        assert!((parse_confidence(r#""confidence": 1.5"#) - 1.0).abs() < f32::EPSILON);
        assert!((parse_confidence(r#""confidence": -0.3"#) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn extract_citations_dedups_preserving_order() {
        let text = "see https://a.com and https://a.com and [1] and [1]";
        let cites = extract_citations(text);
        assert_eq!(cites, vec!["https://a.com".to_string(), "[1]".to_string()]);
    }

    #[test]
    fn extract_citations_empty_when_none_present() {
        assert!(extract_citations("plain text with nothing").is_empty());
    }
}
