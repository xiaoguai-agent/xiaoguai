//! Runner — walks a [`EvalSuite`], runs each [`EvalCase`] against a
//! deterministic [`MockBackend`], collects events + messages, and
//! grades via [`crate::grader::check`].
//!
//! The runner takes an [`EvalAgentBuilder`]: production wiring
//! passes one that constructs a real `ReactAgent` with a `Toolbox`
//! the case may want; tests use [`DefaultEvalAgentBuilder`] which
//! builds an agent with an empty toolbox (enough for outcome
//! graders + transcript graders that don't exercise tools).

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use chrono::Utc;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use xiaoguai_agent::{AgentConfig, AgentError, ReactAgent, Toolbox};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::MockBackend;

use crate::grader::check;
use crate::types::{EvalCase, EvalReport, EvalResult, EvalSuite, MockScript};

/// Default model name passed to the [`AgentConfig`] when the
/// builder constructs an agent. `MockBackend` ignores the field, so
/// the value is purely cosmetic for tracing/log output.
pub const DEFAULT_EVAL_MODEL: &str = "eval-mock";

#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("agent: {0}")]
    Agent(#[from] AgentError),
}

/// Pluggable agent factory. The runner calls
/// [`EvalAgentBuilder::build`] once per case, handing it the
/// case-specific `MockBackend`. Implementations decide what
/// `Toolbox` + `AgentConfig` to wrap around it.
///
/// In v0.11.0 the only production-shaped impl is
/// [`DefaultEvalAgentBuilder`] (empty toolbox + default config). A
/// future v0.11.x slice can ship a builder that instantiates the
/// case's MCP servers in dev-mode so capability evals can exercise
/// real tool dispatch.
#[async_trait]
pub trait EvalAgentBuilder: Send + Sync {
    async fn build(&self, backend: Arc<MockBackend>) -> Result<ReactAgent, RunnerError>;
}

#[derive(Debug, Clone, Default)]
pub struct DefaultEvalAgentBuilder {
    pub max_iterations: u32,
}

impl DefaultEvalAgentBuilder {
    #[must_use]
    pub fn new(max_iterations: u32) -> Self {
        Self { max_iterations }
    }
}

#[async_trait]
impl EvalAgentBuilder for DefaultEvalAgentBuilder {
    async fn build(&self, backend: Arc<MockBackend>) -> Result<ReactAgent, RunnerError> {
        let mut config = AgentConfig::new(DEFAULT_EVAL_MODEL);
        if self.max_iterations > 0 {
            config.max_iterations = self.max_iterations;
        }
        // MockBackend is the universal eval-time backend; empty
        // toolbox is the v0.11.0 default. Builders that need tools
        // override this trait impl wholesale.
        let toolbox = Toolbox::new();
        Ok(ReactAgent::new(backend, toolbox, config))
    }
}

pub struct EvalRunner {
    builder: Arc<dyn EvalAgentBuilder>,
}

impl EvalRunner {
    #[must_use]
    pub fn new(builder: Arc<dyn EvalAgentBuilder>) -> Self {
        Self { builder }
    }

    /// Run every case in `suite` and aggregate the results. Per-case
    /// failures (grader fail, missing script) become `Fail` rows;
    /// only an actual agent-loop crash bubbles up as
    /// [`RunnerError`].
    pub async fn run_suite(&self, suite: &EvalSuite) -> Result<EvalReport, RunnerError> {
        let started_at = Utc::now();
        let mut results = Vec::with_capacity(suite.cases.len());
        for case in &suite.cases {
            let row = self.run_case(case).await?;
            results.push(row);
        }
        let finished_at = Utc::now();
        Ok(EvalReport::from_results(
            suite.name.clone(),
            started_at,
            finished_at,
            results,
        ))
    }

    async fn run_case(&self, case: &EvalCase) -> Result<EvalResult, RunnerError> {
        let started = Instant::now();
        let Some(script) = case.mock_script.as_ref() else {
            return Ok(EvalResult::fail(
                case.id.clone(),
                vec!["case has no mock_script; nothing to drive the agent with".into()],
                0,
                started.elapsed(),
            ));
        };
        if script.turns.is_empty() {
            return Ok(EvalResult::fail(
                case.id.clone(),
                vec!["case mock_script has zero turns".into()],
                0,
                started.elapsed(),
            ));
        }

        let backend = Arc::new(mock_from_script(script));
        let agent = self.builder.build(backend).await?;
        let cancel = CancellationToken::new();
        let (outcome, events) = agent
            .run_to_completion(case.input_messages.clone(), cancel)
            .await?;

        let mut reasons: Vec<String> = case
            .assertions
            .iter()
            .filter_map(|a| check(a, &events, &outcome.messages).err())
            .collect();
        let transcript_len = events.len();
        let duration = started.elapsed();
        if reasons.is_empty() {
            Ok(EvalResult::pass(case.id.clone(), transcript_len, duration))
        } else {
            // Stable order with no surprises — assertions are graded
            // in declaration order.
            reasons.shrink_to_fit();
            Ok(EvalResult::fail(
                case.id.clone(),
                reasons,
                transcript_len,
                duration,
            ))
        }
    }
}

fn mock_from_script(script: &MockScript) -> MockBackend {
    let steps: Vec<ScriptStep> = script
        .turns
        .iter()
        .map(|t| {
            if t.tool_calls.is_empty() {
                ScriptStep::text(t.text.clone())
            } else {
                ScriptStep::tool_calls(t.tool_calls.clone())
            }
        })
        .collect();
    MockBackend::with_script(steps)
}

/// Pretty multi-line summary suitable for the `xiaoguai eval`
/// CLI's stdout. Format is stable but not part of the eval crate's
/// public API guarantee — callers programmatic about results should
/// consume [`EvalReport`] directly.
#[must_use]
pub fn pretty_summary(report: &EvalReport) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(
        out,
        "suite: {} — {}/{} passed ({:.1}%)",
        report.suite,
        report.passed(),
        report.results.len(),
        f64::from(report.pass_rate) * 100.0,
    );
    for row in &report.results {
        match &row.status {
            crate::types::CaseStatus::Pass => {
                let _ = writeln!(
                    out,
                    "  PASS  {:<40} {} events, {} ms",
                    row.case_id, row.transcript_len, row.duration_ms,
                );
            }
            crate::types::CaseStatus::Fail { reasons } => {
                let _ = writeln!(
                    out,
                    "  FAIL  {:<40} {} events, {} ms",
                    row.case_id, row.transcript_len, row.duration_ms,
                );
                for r in reasons {
                    let _ = writeln!(out, "        - {r}");
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Assertion, MockTurn};
    use xiaoguai_llm::Message;

    fn mk_case(id: &str, turns: Vec<MockTurn>, assertions: Vec<Assertion>) -> EvalCase {
        EvalCase {
            id: id.into(),
            input_messages: vec![Message::user("hi")],
            mock_script: Some(MockScript::new(turns)),
            assertions,
            tags: Vec::new(),
        }
    }

    #[tokio::test]
    async fn pass_case_yields_pass_row() {
        let runner = EvalRunner::new(Arc::new(DefaultEvalAgentBuilder::new(2)));
        let case = mk_case(
            "say-hello",
            vec![MockTurn::text("Hello, world!")],
            vec![Assertion::FinalMessageContains {
                text: "Hello".into(),
            }],
        );
        let suite = EvalSuite {
            name: "smoke".into(),
            cases: vec![case],
        };
        let report = runner.run_suite(&suite).await.unwrap();
        assert_eq!(report.results.len(), 1);
        assert!(report.results[0].status.is_pass());
        assert!((report.pass_rate - 1.0).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn fail_case_collects_every_reason() {
        let runner = EvalRunner::new(Arc::new(DefaultEvalAgentBuilder::new(2)));
        let case = mk_case(
            "two-fails",
            vec![MockTurn::text("good morning")],
            vec![
                Assertion::FinalMessageContains {
                    text: "evening".into(),
                },
                Assertion::ToolInvocationCount {
                    tool_name: "search".into(),
                    expected: 1,
                },
            ],
        );
        let suite = EvalSuite {
            name: "regress".into(),
            cases: vec![case],
        };
        let report = runner.run_suite(&suite).await.unwrap();
        assert_eq!(report.results.len(), 1);
        let crate::types::CaseStatus::Fail { reasons } = &report.results[0].status else {
            panic!("expected fail");
        };
        assert_eq!(reasons.len(), 2, "both assertions surface");
        assert!(report.pass_rate < f32::EPSILON);
    }

    #[tokio::test]
    async fn missing_script_marks_fail_without_running_agent() {
        let runner = EvalRunner::new(Arc::new(DefaultEvalAgentBuilder::default()));
        let case = EvalCase {
            id: "no-script".into(),
            input_messages: vec![Message::user("hi")],
            mock_script: None,
            assertions: vec![Assertion::FinalMessageContains { text: "x".into() }],
            tags: Vec::new(),
        };
        let report = runner
            .run_suite(&EvalSuite {
                name: "x".into(),
                cases: vec![case],
            })
            .await
            .unwrap();
        let crate::types::CaseStatus::Fail { reasons } = &report.results[0].status else {
            panic!("expected fail");
        };
        assert!(reasons[0].contains("no mock_script"));
    }

    #[tokio::test]
    async fn pretty_summary_shows_pass_fail_lines() {
        let runner = EvalRunner::new(Arc::new(DefaultEvalAgentBuilder::new(2)));
        let cases = vec![
            mk_case(
                "good",
                vec![MockTurn::text("hello")],
                vec![Assertion::FinalMessageContains {
                    text: "hello".into(),
                }],
            ),
            mk_case(
                "bad",
                vec![MockTurn::text("hello")],
                vec![Assertion::FinalMessageContains {
                    text: "goodbye".into(),
                }],
            ),
        ];
        let report = runner
            .run_suite(&EvalSuite {
                name: "mix".into(),
                cases,
            })
            .await
            .unwrap();
        let s = pretty_summary(&report);
        assert!(s.contains("1/2 passed"));
        assert!(s.contains("PASS"));
        assert!(s.contains("FAIL"));
    }
}
