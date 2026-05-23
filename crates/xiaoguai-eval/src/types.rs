//! Core eval types — `EvalCase`, `Assertion`, `EvalSuite`, results.
//!
//! Designed against roadmap §5.4: **no new database tables.** A case
//! is a scripted [`MockBackend`](xiaoguai_llm::MockBackend) trace plus
//! a list of [`Assertion`]s that grade the transcript + outcome. The
//! eval substrate is the existing `sessions` + `audit_log` tables; a
//! case can be derived from a prod `sessions.id` by extracting its
//! `messages` + `audit_log` rows and translating tool-call sequences
//! into a script (see `tests/regression_from_audit.rs`).

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use xiaoguai_llm::{Message, ToolCallSpec};

/// One scripted assistant turn, sized to drop straight into
/// `xiaoguai_llm::mock::ScriptStep`. We re-declare the shape locally
/// so eval YAML doesn't carry the upstream's `FinishReason` (which
/// is a runtime concern; eval cases only care about text vs tool
/// calls).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MockTurn {
    /// Streamed text the model "emits". Empty when the turn is only
    /// tool calls.
    #[serde(default)]
    pub text: String,
    /// Tool calls the model issues. Empty for plain-text turns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallSpec>,
}

impl MockTurn {
    #[must_use]
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            text: content.into(),
            tool_calls: Vec::new(),
        }
    }

    #[must_use]
    pub fn tool_calls(calls: Vec<ToolCallSpec>) -> Self {
        Self {
            text: String::new(),
            tool_calls: calls,
        }
    }
}

/// Full mock script for one case — a sequence of model turns the
/// `MockBackend` will replay in order. The last turn replays
/// indefinitely if the agent loops past it; same semantics as the
/// upstream `MockBackend::with_script`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MockScript {
    pub turns: Vec<MockTurn>,
}

impl MockScript {
    #[must_use]
    pub fn new(turns: Vec<MockTurn>) -> Self {
        Self { turns }
    }
}

/// Tool-call pattern used by [`Assertion::ToolCallSequence`].
///
/// `arguments_json_substring` keeps the YAML readable: a case can
/// pin "the arguments contained `foo`" without committing to exact
/// JSON formatting the model may have produced. Empty string
/// matches any arguments. We deliberately don't ship a full JSON-
/// path matcher in v0.11.0 — substring + tool name is enough for
/// regression cases extracted from real runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallPattern {
    pub tool_name: String,
    #[serde(default)]
    pub arguments_json_substring: String,
}

/// Agent-event pattern used by [`Assertion::AgentEventSequence`].
/// We match on the `snake_case` event tag (the `type` discriminant
/// `AgentEvent` serializes to). Keeps the YAML simple and the
/// matcher robust to event-payload churn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEventPattern {
    pub event_type: String,
}

/// Grader applied to the recorded run. Each variant must be cheap
/// to evaluate: we walk every assertion once after the agent loop
/// finishes, so allocation here lives on the eval critical path.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Assertion {
    /// The final assistant message's text content contains `text`.
    FinalMessageContains { text: String },

    /// The final assistant message's text content matches `pattern`
    /// (full regex; not anchored). Invalid regex fails the case
    /// with a clear reason rather than panicking — the assertion is
    /// the grader, not the runner.
    FinalMessageRegex { pattern: String },

    /// Exactly `expected` calls to `tool_name` were observed across
    /// the full transcript (counted from
    /// `AgentEvent::ToolCallStarted`).
    ToolInvocationCount { tool_name: String, expected: usize },

    /// Observed `AgentEvent` types match `expected` in order. Uses
    /// the serialized `snake_case` discriminant (e.g. `text_delta`,
    /// `tool_call_started`, `done`). Subsequence match — `expected`
    /// must appear in order but other events may interleave. Strict
    /// equality is too brittle for streaming `text_delta`s.
    AgentEventSequence { expected: Vec<AgentEventPattern> },

    /// Observed tool calls match `expected` in order, by tool name +
    /// optional argument substring. Strict subsequence (other tool
    /// calls allowed between matches).
    ToolCallSequence { expected: Vec<ToolCallPattern> },
}

/// One eval case. `mock_script` is optional only so YAML cases that
/// model "no model interaction expected" (e.g. a case asserting
/// startup state) can omit it; the runner refuses to execute such a
/// case (returns a clear failure reason).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalCase {
    pub id: String,
    pub input_messages: Vec<Message>,
    #[serde(default)]
    pub mock_script: Option<MockScript>,
    pub assertions: Vec<Assertion>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Group of cases run together. The runner walks `cases` in order;
/// failures don't stop the suite (every case gets a result row).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalSuite {
    pub name: String,
    pub cases: Vec<EvalCase>,
}

/// Per-case verdict. `Fail` carries every reason — we don't bail at
/// the first failure so a case-author sees the full picture.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CaseStatus {
    Pass,
    Fail { reasons: Vec<String> },
}

impl CaseStatus {
    #[must_use]
    pub fn is_pass(&self) -> bool {
        matches!(self, Self::Pass)
    }
}

/// One row in the per-suite report. `transcript_len` is the count of
/// `AgentEvent`s observed; `duration_ms` is wall-clock time for the
/// case (executor + assertions, not YAML load).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalResult {
    pub case_id: String,
    #[serde(flatten)]
    pub status: CaseStatus,
    pub transcript_len: usize,
    pub duration_ms: u64,
}

impl EvalResult {
    #[must_use]
    pub fn pass(case_id: impl Into<String>, transcript_len: usize, duration: Duration) -> Self {
        Self {
            case_id: case_id.into(),
            status: CaseStatus::Pass,
            transcript_len,
            duration_ms: duration_ms_u64(duration),
        }
    }

    #[must_use]
    pub fn fail(
        case_id: impl Into<String>,
        reasons: Vec<String>,
        transcript_len: usize,
        duration: Duration,
    ) -> Self {
        Self {
            case_id: case_id.into(),
            status: CaseStatus::Fail { reasons },
            transcript_len,
            duration_ms: duration_ms_u64(duration),
        }
    }
}

fn duration_ms_u64(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Aggregate report for one suite run. `pass_rate` is in [0, 1].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalReport {
    pub suite: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub results: Vec<EvalResult>,
    pub pass_rate: f32,
}

impl EvalReport {
    #[must_use]
    pub fn from_results(
        suite: impl Into<String>,
        started_at: DateTime<Utc>,
        finished_at: DateTime<Utc>,
        results: Vec<EvalResult>,
    ) -> Self {
        let total = results.len();
        let passed = results.iter().filter(|r| r.status.is_pass()).count();
        #[allow(clippy::cast_precision_loss)]
        let rate = if total == 0 {
            0.0_f32
        } else {
            passed as f32 / total as f32
        };
        Self {
            suite: suite.into(),
            started_at,
            finished_at,
            results,
            pass_rate: rate,
        }
    }

    #[must_use]
    pub fn passed(&self) -> usize {
        self.results.iter().filter(|r| r.status.is_pass()).count()
    }

    #[must_use]
    pub fn failed(&self) -> usize {
        self.results.len() - self.passed()
    }
}
