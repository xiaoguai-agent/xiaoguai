//! Eval substrate for xiaoguai (v0.11.0).
//!
//! Roadmap §5.4 fixes the contract: *no new DB tables*. An eval
//! case is `(input_messages, mock_script, assertions)`; the
//! substrate is the existing `sessions` + `audit_log` tables plus
//! `xiaoguai_llm::MockBackend` for deterministic traces.
//!
//! Three pieces, mirroring Anthropic's "Demystifying Evals for AI
//! Agents":
//!
//! 1. **Outcome graders** — [`Assertion::FinalMessageContains`] /
//!    [`Assertion::FinalMessageRegex`] /
//!    [`Assertion::ToolInvocationCount`]. Cheap to evaluate; the
//!    bulk of regression cases extracted from real bugs land here.
//! 2. **Transcript graders** — [`Assertion::AgentEventSequence`] /
//!    [`Assertion::ToolCallSequence`]. Subsequence-match the
//!    streamed `AgentEvent`s and tool calls, so streaming text
//!    deltas don't make assertions brittle.
//! 3. **Runner + report** — [`EvalRunner`] walks an [`EvalSuite`]
//!    against a deterministic [`MockBackend`](xiaoguai_llm::MockBackend)
//!    via a pluggable [`EvalAgentBuilder`]; the per-case verdicts
//!    aggregate into an [`EvalReport`] with `pass_rate` and a JSON
//!    serializer.
//!
//! The CLI binding (`xiaoguai eval run --suite <name>`) lives in
//! `xiaoguai-cli` and delegates straight into
//! [`EvalRunner::run_suite`] + [`EvalReport::write_json`].
//!
//! Deferred to v0.11.1+:
//!
//! * `Assertion::RepoStateMatches(JsonPath, Value)` — needs a
//!   well-defined "in-memory repo snapshot" surface that v0.11.0
//!   doesn't have; revisit alongside the audit-first console.
//! * PG-backed runner that recreates a case from a `sessions.id` +
//!   `audit_log` rows automatically (the
//!   `tests/regression_from_audit.rs` test sketches the manual
//!   shape).
//! * Capability suites that exercise a real `Toolbox` end-to-end
//!   (today `DefaultEvalAgentBuilder` ships an empty `Toolbox`).

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod grader;
pub mod report;
pub mod runner;
pub mod suite;
pub mod types;

pub use grader::check;
pub use report::ReportError;
pub use runner::{
    pretty_summary, DefaultEvalAgentBuilder, EvalAgentBuilder, EvalRunner, RunnerError,
    DEFAULT_EVAL_MODEL,
};
pub use suite::SuiteError;
pub use types::{
    AgentEventPattern, Assertion, CaseStatus, EvalCase, EvalReport, EvalResult, EvalSuite,
    MockScript, MockTurn, ToolCallPattern,
};
