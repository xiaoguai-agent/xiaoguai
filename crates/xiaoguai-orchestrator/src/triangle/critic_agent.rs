//! Sprint-9 S9-4 — `CriticAgent`.
//!
//! Phase C: one LLM call per `WorkerResult` → [`Verdict`]
//! (`Approve` / `RequestRevision` / `Reject`). Per
//! `xiaoguai-agent-design/docs/lld/lld-orchestrator.md` §4.4 the
//! Critic's job is small + structured + tightly budgeted (10 % default
//! per `TriangleBudget`). Same one-shot pattern as
//! [`super::planner_agent::PlannerAgent`]:
//!
//! - Direct `LlmBackend::chat_stream` call (no `ReactAgent` — zero
//!   tool calls; same Plan-§6-Deviation rationale).
//! - JSON output parsed into [`Verdict`]; one retry on malformed.
//!
//! Pre-LLM gates intentionally NOT implemented (e.g., short-circuit
//! when `WorkerResult.artefact` is `None`, or when `confidence <
//! min_confidence`). Rationale: keeping all judgment in one place
//! (the LLM) prevents Critic Rust-vs-LLM divergence — if the Rust
//! pre-gate says Reject but the LLM would have said Approve, we have
//! a confusing debug surface. The LLM sees enough context to make
//! the same calls.
//!
//! Scratchpad is read-only here (see `Scratchpad` doc — Critic gets
//! `&Scratchpad`, never `&mut`).

use std::sync::Arc;

use futures::StreamExt;
use serde::Deserialize;
use thiserror::Error;
use xiaoguai_llm::{ChatRequest, LlmBackend, LlmError, Message};

use super::plan::AcceptanceCriteria;
use super::scratchpad::Scratchpad;
use super::verdict::Verdict;
use super::worker_agent::WorkerResult;

/// Maximum scratchpad entries to surface in the Critic prompt. Older
/// entries are silently dropped — keeping the prompt small matters for
/// the 10 % budget allocation.
const SCRATCHPAD_TAIL: usize = 3;

const MODEL_PLACEHOLDER: &str = "critic";

const CRITIC_INSTRUCTIONS: &str = r#"
You review a Worker's result against rubric-based acceptance criteria.
Emit a JSON object with EXACTLY one of these shapes:

  {"kind": "approve", "reason": "<why it passes>"}
  {"kind": "request_revision", "feedback": "<what's missing or wrong>"}
  {"kind": "reject", "reason": "<why the task is fundamentally wrong>"}

Use:
- `approve` when the rubric is fully met. Worker's artefact is taken.
- `request_revision` when the rubric is *almost* met and the Worker
  could likely fix it with feedback. The Worker will re-run with your
  `feedback` injected as additional context (max 3 revisions per task).
- `reject` when the task itself is wrong — e.g., the goal does not
  match the artefact's domain, or the rubric cannot be satisfied by any
  Worker on this task. Rejected tasks return to the Planner for a
  replan.

Do NOT wrap the JSON in markdown fences. Plain JSON only.
"#;

#[derive(Debug, Error)]
pub enum CriticError {
    #[error("backend failed: {0}")]
    LlmError(#[from] LlmError),
    #[error("malformed verdict JSON after {attempts} attempts; last error: {last_error}")]
    MalformedJson { attempts: u32, last_error: String },
    /// Reserved for the pattern runner; the agent itself does not
    /// raise this today.
    #[error("critic budget exhausted")]
    BudgetExhausted,
}

/// Single-LLM-call Critic. Stateless across `.review()` calls.
pub struct CriticAgent {
    inner: Arc<dyn LlmBackend>,
    persona_prompt: String,
    max_retry: u32,
}

impl CriticAgent {
    /// `max_retry` defaults to 1 (so worst-case 2 LLM calls).
    #[must_use]
    pub fn new(inner: Arc<dyn LlmBackend>, persona_prompt: String) -> Self {
        Self {
            inner,
            persona_prompt,
            max_retry: 1,
        }
    }

    /// Customise the retry budget. `0` disables retry.
    #[must_use]
    pub fn with_max_retry(mut self, max_retry: u32) -> Self {
        self.max_retry = max_retry;
        self
    }

    /// Review a `WorkerResult` against `criteria`, also showing the
    /// Critic the tail of the Worker's `Scratchpad` for context.
    ///
    /// # Errors
    /// - `LlmError` — backend failure (network/quota/…).
    /// - `MalformedJson` — all attempts returned non-parseable text.
    pub async fn review(
        &self,
        worker_result: &WorkerResult,
        criteria: &AcceptanceCriteria,
        scratchpad: &Scratchpad,
    ) -> Result<Verdict, CriticError> {
        let max_attempts = self.max_retry + 1;
        let mut last_error: Option<String> = None;

        for _attempt in 1..=max_attempts {
            let system = build_system_prompt(
                &self.persona_prompt,
                criteria,
                scratchpad,
                last_error.as_deref(),
            );
            let user = render_worker_result(worker_result);
            let messages = vec![Message::system(system), Message::user(user)];
            let request = ChatRequest::new(MODEL_PLACEHOLDER, messages);

            let raw = collect_stream(self.inner.as_ref(), request).await?;
            match parse_verdict(&raw) {
                Ok(v) => return Ok(v),
                Err(e) => last_error = Some(e),
            }
        }

        Err(CriticError::MalformedJson {
            attempts: max_attempts,
            last_error: last_error.unwrap_or_else(|| "no error captured".into()),
        })
    }
}

fn build_system_prompt(
    persona_prompt: &str,
    criteria: &AcceptanceCriteria,
    scratchpad: &Scratchpad,
    retry_context: Option<&str>,
) -> String {
    let mut s = String::with_capacity(1024);
    s.push_str(persona_prompt);
    s.push_str("\n\n");
    s.push_str(CRITIC_INSTRUCTIONS);
    s.push_str("\n\n## Acceptance criteria\n");
    s.push_str("Rubric: ");
    s.push_str(&criteria.rubric);
    if let Some(p) = &criteria.required_citation_pattern {
        s.push_str("\nRequired citation pattern (substring): ");
        s.push_str(p);
    }
    if let Some(c) = criteria.min_confidence {
        s.push_str(&format!("\nMinimum self-reported confidence: {c:.2}"));
    }
    s.push_str("\n\n## Worker scratchpad tail\n");
    let entries = scratchpad.entries();
    let start = entries.len().saturating_sub(SCRATCHPAD_TAIL);
    if entries[start..].is_empty() {
        s.push_str("(scratchpad empty)\n");
    } else {
        for (i, e) in entries[start..].iter().enumerate() {
            s.push_str(&format!("entry {i}: {}\n", e.content));
        }
    }
    if let Some(err) = retry_context {
        s.push_str("\n## Previous attempt failed to parse\n");
        s.push_str(err);
        s.push_str("\nFix your JSON and produce a valid verdict.");
    }
    s
}

fn render_worker_result(r: &WorkerResult) -> String {
    let artefact = r.artefact.as_deref().unwrap_or("(no artefact produced)");
    let citations = if r.citations.is_empty() {
        "(none extracted)".to_string()
    } else {
        r.citations.join(", ")
    };
    format!(
        "task_id: {tid}\n\
         confidence: {conf:.2}\n\
         iterations: {iter}\n\
         cost_tokens: {cost}\n\
         stop_reason: {stop:?}\n\
         citations: {cits}\n\
         artefact:\n{art}",
        tid = r.task_id,
        conf = r.confidence,
        iter = r.iterations,
        cost = r.cost_tokens,
        stop = r.stop_reason,
        cits = citations,
        art = artefact,
    )
}

async fn collect_stream(
    backend: &dyn LlmBackend,
    request: ChatRequest,
) -> Result<String, LlmError> {
    let mut stream = backend.chat_stream(request).await?;
    let mut out = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        out.push_str(&chunk.delta);
    }
    Ok(out)
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RawVerdict {
    Approve { reason: String },
    RequestRevision { feedback: String },
    Reject { reason: String },
}

impl From<RawVerdict> for Verdict {
    fn from(r: RawVerdict) -> Self {
        match r {
            RawVerdict::Approve { reason } => Verdict::Approve { reason },
            RawVerdict::RequestRevision { feedback } => Verdict::RequestRevision { feedback },
            RawVerdict::Reject { reason } => Verdict::Reject { reason },
        }
    }
}

fn parse_verdict(raw: &str) -> Result<Verdict, String> {
    let trimmed = raw.trim();
    // Strip accidental code fences — some LLMs emit them despite the
    // instruction. Cheap to be tolerant here.
    let cleaned = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|s| s.strip_suffix("```").unwrap_or(s))
        .unwrap_or(trimmed)
        .trim();
    let raw: RawVerdict = serde_json::from_str(cleaned).map_err(|e| e.to_string())?;
    Ok(raw.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triangle::plan::TaskId;
    use crate::triangle::worker_agent::{WorkerResult, WorkerStopReason};
    use async_trait::async_trait;
    use futures::stream;
    use xiaoguai_llm::backend::ChatStream;
    use xiaoguai_llm::{ChatChunk, FinishReason};

    /// Mock backend that returns a canned sequence of responses.
    struct CannedBackend {
        responses: parking_lot::Mutex<Vec<String>>,
        captured: parking_lot::Mutex<Vec<ChatRequest>>,
    }

    impl CannedBackend {
        fn new(responses: Vec<&str>) -> Arc<Self> {
            Arc::new(Self {
                responses: parking_lot::Mutex::new(
                    responses.into_iter().map(String::from).collect(),
                ),
                captured: parking_lot::Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait]
    impl LlmBackend for CannedBackend {
        fn name(&self) -> &'static str {
            "canned-critic-test"
        }
        async fn chat_stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
            self.captured.lock().push(req);
            let next = self
                .responses
                .lock()
                .pop()
                .ok_or_else(|| LlmError::Provider("no more canned responses".into()))?;
            let chunk = ChatChunk {
                delta: next,
                reasoning_delta: None,
                tool_calls: vec![],
                finish_reason: Some(FinishReason::Stop),
                done: true,
            };
            Ok(Box::pin(stream::iter(vec![Ok(chunk)])))
        }
    }

    fn ac() -> AcceptanceCriteria {
        AcceptanceCriteria {
            rubric: "answer mentions cluster id and is non-empty".into(),
            required_citation_pattern: None,
            min_confidence: Some(0.5),
        }
    }

    fn worker_result(artefact: Option<&str>, confidence: f32) -> WorkerResult {
        WorkerResult {
            task_id: TaskId::new(),
            artefact: artefact.map(String::from),
            citations: vec![],
            confidence,
            cost_tokens: 100,
            iterations: 1,
            stop_reason: WorkerStopReason::Completed,
        }
    }

    fn scratchpad_with(content: &[&str]) -> Scratchpad {
        let id = TaskId::new();
        let mut s = Scratchpad::new(id);
        for c in content {
            s.append(id, (*c).into(), Some(10)).unwrap();
        }
        s
    }

    #[tokio::test]
    async fn approve_happy_path() {
        let backend = CannedBackend::new(vec![
            r#"{"kind":"approve","reason":"cluster id present and rubric met"}"#,
        ]);
        let critic = CriticAgent::new(backend.clone() as Arc<dyn LlmBackend>, "Critic".into());
        let result = worker_result(Some("Cluster prod-east-7 is healthy"), 0.9);
        let v = critic
            .review(&result, &ac(), &scratchpad_with(&["draft"]))
            .await
            .unwrap();
        assert!(matches!(v, Verdict::Approve { .. }));
        assert_eq!(v.explanation(), "cluster id present and rubric met");
    }

    #[tokio::test]
    async fn request_revision_path() {
        let backend = CannedBackend::new(vec![
            r#"{"kind":"request_revision","feedback":"missing cluster id"}"#,
        ]);
        let critic = CriticAgent::new(backend as Arc<dyn LlmBackend>, "Critic".into());
        let result = worker_result(Some("the cluster is fine"), 0.8);
        let v = critic
            .review(&result, &ac(), &scratchpad_with(&["draft"]))
            .await
            .unwrap();
        assert!(matches!(v, Verdict::RequestRevision { .. }));
    }

    #[tokio::test]
    async fn reject_path() {
        let backend = CannedBackend::new(vec![
            r#"{"kind":"reject","reason":"task is asking for k8s but artefact is about postgres"}"#,
        ]);
        let critic = CriticAgent::new(backend as Arc<dyn LlmBackend>, "Critic".into());
        let result = worker_result(Some("Postgres replication lag is 5ms"), 0.6);
        let v = critic
            .review(&result, &ac(), &scratchpad_with(&[]))
            .await
            .unwrap();
        assert!(matches!(v, Verdict::Reject { .. }));
    }

    #[tokio::test]
    async fn malformed_then_valid_retries() {
        // 1st response (popped LAST) malformed; 2nd valid.
        let backend = CannedBackend::new(vec![
            r#"{"kind":"approve","reason":"ok"}"#, // popped first (Vec::pop is LIFO)
            r#"not json at all"#,                  // popped first call
        ]);
        let critic = CriticAgent::new(backend as Arc<dyn LlmBackend>, "Critic".into());
        let result = worker_result(Some("ok"), 0.9);
        let v = critic
            .review(&result, &ac(), &scratchpad_with(&[]))
            .await
            .unwrap();
        assert!(matches!(v, Verdict::Approve { .. }));
    }

    #[tokio::test]
    async fn two_malformed_returns_malformed_json() {
        let backend = CannedBackend::new(vec!["second garbage", "first garbage"]);
        let critic = CriticAgent::new(backend as Arc<dyn LlmBackend>, "Critic".into());
        let result = worker_result(Some("art"), 0.5);
        let err = critic
            .review(&result, &ac(), &scratchpad_with(&[]))
            .await
            .unwrap_err();
        match err {
            CriticError::MalformedJson { attempts, .. } => assert_eq!(attempts, 2),
            other => panic!("expected MalformedJson, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn code_fence_around_json_is_tolerated() {
        let backend = CannedBackend::new(vec![
            "```json\n{\"kind\":\"approve\",\"reason\":\"fenced\"}\n```",
        ]);
        let critic = CriticAgent::new(backend as Arc<dyn LlmBackend>, "Critic".into());
        let result = worker_result(Some("art"), 0.7);
        let v = critic
            .review(&result, &ac(), &scratchpad_with(&[]))
            .await
            .unwrap();
        assert!(matches!(v, Verdict::Approve { .. }));
    }

    #[tokio::test]
    async fn persona_and_criteria_appear_in_prompt() {
        let backend = CannedBackend::new(vec![r#"{"kind":"approve","reason":"ok"}"#]);
        let critic = CriticAgent::new(
            backend.clone() as Arc<dyn LlmBackend>,
            "You are the senior reviewer.".into(),
        );
        let result = worker_result(Some("art"), 0.8);
        critic
            .review(&result, &ac(), &scratchpad_with(&["entry-a", "entry-b"]))
            .await
            .unwrap();
        let captured = backend.captured.lock();
        let system = &captured[0].messages[0].content;
        assert!(system.contains("You are the senior reviewer."));
        assert!(system.contains("answer mentions cluster id"));
        assert!(system.contains("Minimum self-reported confidence: 0.50"));
        assert!(system.contains("entry-a") || system.contains("entry-b"));
    }

    #[tokio::test]
    async fn scratchpad_tail_caps_at_three_entries() {
        let backend = CannedBackend::new(vec![r#"{"kind":"approve","reason":"ok"}"#]);
        let critic = CriticAgent::new(backend.clone() as Arc<dyn LlmBackend>, "C".into());
        let result = worker_result(Some("art"), 0.8);
        // 5 entries; only last 3 should appear in prompt.
        let s = scratchpad_with(&["e1", "e2", "e3", "e4", "e5"]);
        critic.review(&result, &ac(), &s).await.unwrap();
        let captured = backend.captured.lock();
        let system = &captured[0].messages[0].content;
        assert!(!system.contains("e1"), "old entry e1 should be dropped");
        assert!(!system.contains("e2"), "old entry e2 should be dropped");
        assert!(system.contains("e3"));
        assert!(system.contains("e4"));
        assert!(system.contains("e5"));
    }

    #[tokio::test]
    async fn worker_result_no_artefact_renders_placeholder() {
        let backend =
            CannedBackend::new(vec![r#"{"kind":"reject","reason":"no artefact produced"}"#]);
        let critic = CriticAgent::new(backend.clone() as Arc<dyn LlmBackend>, "C".into());
        let result = worker_result(None, 0.0);
        let v = critic
            .review(&result, &ac(), &scratchpad_with(&[]))
            .await
            .unwrap();
        assert!(matches!(v, Verdict::Reject { .. }));
        let captured = backend.captured.lock();
        let user = &captured[0].messages[1].content;
        assert!(user.contains("(no artefact produced)"));
    }

    #[tokio::test]
    async fn worker_result_citations_rendered_into_user_message() {
        let backend = CannedBackend::new(vec![r#"{"kind":"approve","reason":"ok"}"#]);
        let critic = CriticAgent::new(backend.clone() as Arc<dyn LlmBackend>, "C".into());
        let mut result = worker_result(Some("art"), 0.9);
        result.citations = vec!["https://example.com/a".into(), "[1]".into()];
        critic
            .review(&result, &ac(), &scratchpad_with(&[]))
            .await
            .unwrap();
        let captured = backend.captured.lock();
        let user = &captured[0].messages[1].content;
        assert!(user.contains("https://example.com/a"));
        assert!(user.contains("[1]"));
    }
}
