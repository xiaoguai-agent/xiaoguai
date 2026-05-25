//! Challenger — institutional red-team middleware for the Supervisor.
//!
//! Before a high-risk step is dispatched, the Supervisor routes it through
//! a `Challenger`.  The challenger asks "what could go wrong?" and "what
//! assumptions does this step rest on?" and returns a `Critique`.
//!
//! ## Verdict semantics
//!
//! - `Accept`          — step proceeds as normal.
//! - `RequestRevision` — the planner is re-asked with the critique text
//!   injected as context.  A loop counter prevents infinite revision.
//! - `Reject`          — step is skipped; the critique is recorded on
//!   the `StepResult`.
//!
//! ## Design
//!
//! The `Challenger` trait is intentionally thin so it can be implemented by
//! a `MockChallenger` (scripted verdicts), an `LlmChallenger` (LLM-backed),
//! or any future policy engine.

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::error::OrchestratorError;
use crate::plan::PlanStep;

// ── Public types ──────────────────────────────────────────────────────────────

/// The decision made by a challenger about a proposed plan step.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// The step is fine — proceed.
    Accept,
    /// The step has issues; ask the planner to revise using the critique.
    RequestRevision,
    /// The step must not be executed; skip it and record the critique.
    Reject,
}

/// Structured feedback produced by a challenger.
#[derive(Debug, Clone)]
pub struct Critique {
    /// Overall verdict for the proposed step.
    pub verdict: Verdict,
    /// Human-readable reasons or risk factors.
    pub reasons: Vec<String>,
    /// Aggregate risk score in `[0.0, 1.0]`.  `1.0` = maximum risk.
    pub risk_score: f64,
}

impl Critique {
    /// Convenience: build an Accept critique with no reasons and zero risk.
    pub fn accept() -> Self {
        Self {
            verdict: Verdict::Accept,
            reasons: vec![],
            risk_score: 0.0,
        }
    }

    /// Convenience: build a Reject critique.
    pub fn reject(reasons: Vec<String>, risk_score: f64) -> Self {
        Self {
            verdict: Verdict::Reject,
            reasons,
            risk_score,
        }
    }

    /// Convenience: build a `RequestRevision` critique.
    pub fn revise(reasons: Vec<String>, risk_score: f64) -> Self {
        Self {
            verdict: Verdict::RequestRevision,
            reasons,
            risk_score,
        }
    }

    /// Format the critique as a short context string for the planner.
    pub fn to_context_string(&self) -> String {
        format!(
            "[CRITIQUE risk={:.2}] {}",
            self.risk_score,
            self.reasons.join("; ")
        )
    }
}

// ── Trait ─────────────────────────────────────────────────────────────────────

/// Strategy interface for challenging a proposed plan step.
#[async_trait]
pub trait Challenger: Send + Sync {
    /// Analyse the proposed step and return a structured critique.
    async fn critique(&self, proposed: &PlanStep) -> Result<Critique, OrchestratorError>;
}

// ── MockChallenger ────────────────────────────────────────────────────────────

/// A scripted challenger that returns pre-loaded verdicts in FIFO order.
///
/// Useful for deterministic unit tests.  When the queue is exhausted it
/// defaults to `Critique::accept()`.
pub struct MockChallenger {
    responses: Mutex<VecDeque<Critique>>,
}

impl MockChallenger {
    /// Create with an empty queue (all calls default to Accept).
    pub fn new() -> Self {
        Self {
            responses: Mutex::new(VecDeque::new()),
        }
    }

    /// Push a scripted `Critique` to the back of the FIFO queue.
    #[must_use]
    pub fn push(self, critique: Critique) -> Self {
        self.responses.lock().unwrap().push_back(critique);
        self
    }
}

impl Default for MockChallenger {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Challenger for MockChallenger {
    async fn critique(&self, _proposed: &PlanStep) -> Result<Critique, OrchestratorError> {
        Ok(self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(Critique::accept))
    }
}

// ── LlmChallenger ─────────────────────────────────────────────────────────────

/// An LLM-backed challenger that asks "what could go wrong?" and "what
/// assumptions does this rest on?".
///
/// The `llm` field is an opaque string-in / string-out function so this crate
/// does not take a hard dependency on `xiaoguai-llm`.  Pass a closure that
/// calls the LLM backend you want.
///
/// ```rust,ignore
/// use std::sync::Arc;
/// use xiaoguai_orchestrator::challenger::LlmChallenger;
///
/// let challenger = LlmChallenger::new(
///     "You are a rigorous risk analyst. Given the proposed action below,\n\
///      identify what could go wrong and what assumptions it relies on.\n\
///      Reply with: VERDICT: Accept|RequestRevision|Reject\n\
///      RISK_SCORE: 0.0-1.0\n\
///      REASONS: bullet list\n\n\
///      Proposed action: {description}",
///     Arc::new(|prompt: String| async move { Ok("VERDICT: Accept\nRISK_SCORE: 0.1\nREASONS: - none".to_string()) }),
/// );
/// ```
pub struct LlmChallenger<F> {
    /// Template with a `{description}` placeholder.
    pub prompt_template: String,
    /// Callable that accepts a rendered prompt and returns the LLM response.
    pub llm: std::sync::Arc<F>,
}

impl<F, Fut> LlmChallenger<F>
where
    F: Fn(String) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<String, OrchestratorError>> + Send,
{
    pub fn new(prompt_template: impl Into<String>, llm: std::sync::Arc<F>) -> Self {
        Self {
            prompt_template: prompt_template.into(),
            llm,
        }
    }
}

#[async_trait]
impl<F, Fut> Challenger for LlmChallenger<F>
where
    F: Fn(String) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<String, OrchestratorError>> + Send,
{
    async fn critique(&self, proposed: &PlanStep) -> Result<Critique, OrchestratorError> {
        let prompt = self
            .prompt_template
            .replace("{description}", &proposed.description);
        let response = (self.llm)(prompt).await?;
        Ok(parse_llm_critique(&response))
    }
}

/// Parse the structured LLM response into a `Critique`.
///
/// Expected format (whitespace-tolerant):
/// ```text
/// VERDICT: Accept | RequestRevision | Reject
/// RISK_SCORE: 0.0–1.0
/// REASONS: - reason one
///          - reason two
/// ```
fn parse_llm_critique(response: &str) -> Critique {
    let verdict = if response.contains("VERDICT: Reject") {
        Verdict::Reject
    } else if response.contains("VERDICT: RequestRevision") {
        Verdict::RequestRevision
    } else {
        // Default Accept for any unrecognised or Accept response.
        Verdict::Accept
    };

    let risk_score = response
        .lines()
        .find(|l| l.trim_start().starts_with("RISK_SCORE:"))
        .and_then(|l| l.split_once(':').map(|x| x.1))
        .and_then(|s| s.trim().parse::<f64>().ok())
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);

    let reasons: Vec<String> = response
        .lines()
        .skip_while(|l| !l.trim_start().starts_with("REASONS:"))
        .skip(1) // skip the "REASONS:" line itself
        .map(|l| l.trim_start_matches('-').trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    Critique {
        verdict,
        reasons,
        risk_score,
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::PlanStep;

    fn step(id: &str) -> PlanStep {
        PlanStep::new(id, format!("Execute {id}"), vec![])
    }

    #[tokio::test]
    async fn mock_challenger_returns_scripted_verdicts() {
        let ch = MockChallenger::new()
            .push(Critique::reject(vec!["risky".to_string()], 0.9))
            .push(Critique::accept());

        let first = ch.critique(&step("s1")).await.unwrap();
        assert_eq!(first.verdict, Verdict::Reject);
        assert!((first.risk_score - 0.9).abs() < 1e-9);
        assert_eq!(first.reasons, vec!["risky".to_string()]);

        let second = ch.critique(&step("s2")).await.unwrap();
        assert_eq!(second.verdict, Verdict::Accept);
    }

    #[tokio::test]
    async fn mock_challenger_defaults_to_accept_when_queue_empty() {
        let ch = MockChallenger::new();
        let c = ch.critique(&step("s")).await.unwrap();
        assert_eq!(c.verdict, Verdict::Accept);
        assert!((c.risk_score - 0.0).abs() < 1e-9);
    }

    #[test]
    fn parse_llm_reject_response() {
        let resp = "VERDICT: Reject\nRISK_SCORE: 0.85\nREASONS:\n- funds may be insufficient\n- no rollback path";
        let c = parse_llm_critique(resp);
        assert_eq!(c.verdict, Verdict::Reject);
        assert!((c.risk_score - 0.85).abs() < 1e-6);
        assert_eq!(c.reasons.len(), 2);
    }

    #[test]
    fn parse_llm_revision_response() {
        let resp = "VERDICT: RequestRevision\nRISK_SCORE: 0.5\nREASONS:\n- amount unvalidated";
        let c = parse_llm_critique(resp);
        assert_eq!(c.verdict, Verdict::RequestRevision);
    }

    #[test]
    fn parse_llm_accept_fallback_on_unknown() {
        let c = parse_llm_critique("This looks fine.");
        assert_eq!(c.verdict, Verdict::Accept);
        assert!((c.risk_score - 0.0).abs() < 1e-9);
    }

    #[test]
    fn critique_context_string_format() {
        let c = Critique::reject(vec!["no rollback".to_string()], 0.8);
        let s = c.to_context_string();
        assert!(s.contains("0.80"));
        assert!(s.contains("no rollback"));
    }

    #[tokio::test]
    async fn llm_challenger_parses_response() {
        use std::sync::Arc;

        fn make_llm(
            response: &'static str,
        ) -> impl Fn(
            String,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<String, OrchestratorError>> + Send>,
        > + Send
               + Sync
               + 'static {
            move |_prompt: String| Box::pin(async move { Ok(response.to_string()) })
        }

        let challenger = LlmChallenger::new(
            "Check: {description}",
            Arc::new(make_llm(
                "VERDICT: Reject\nRISK_SCORE: 0.9\nREASONS:\n- bad idea",
            )),
        );
        let c = challenger.critique(&step("pay")).await.unwrap();
        assert_eq!(c.verdict, Verdict::Reject);
        assert!((c.risk_score - 0.9).abs() < 1e-6);
    }
}
