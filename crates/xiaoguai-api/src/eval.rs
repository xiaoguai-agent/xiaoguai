//! v0.11.2 — eval pane substrate.
//!
//! Three admin endpoints close the audit ↔ eval loop the roadmap §3
//! v0.11.2 row sketches:
//!
//! * `POST /v1/admin/eval/run` — run a `.eval.yaml` suite end-to-end
//!   against the same [`EvalRunner`](xiaoguai_eval::EvalRunner) the CLI
//!   uses, returning the full [`EvalReport`](xiaoguai_eval::EvalReport).
//! * `GET  /v1/admin/eval/suites` — enumerate suites available on disk
//!   under the configured directory.
//! * `POST /v1/admin/eval/case-from-session` — project a prod
//!   `sessions.id` (its message history + `tool.invoke` audit rows) into
//!   a ready-to-edit `EvalCase` YAML string the operator pastes into a
//!   new `.eval.yaml` file.
//!
//! Design notes:
//!
//! * The api crate stays storage-agnostic. The session→case translation
//!   takes a [`CaseFromSessionSource`] trait whose production
//!   implementation lives in `xiaoguai-core` (PG-backed), mirroring how
//!   `AuditReader` / `TodayReader` are layered.
//! * Suite execution is intentionally synchronous: the CLAUDE.md
//!   v0.11.2 contract says "suites are small; SSE progress streaming
//!   deferred". A static cap (max cases + total wall-clock) guards
//!   against pathological inputs.
//! * Reports are not persisted server-side — the v0.11.0 plan deferred a
//!   `pg_eval_reports` table. The console caches "last run" in
//!   localStorage; durable history defers to v0.12.0.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use xiaoguai_eval::{EvalCase, EvalReport, EvalRunner, EvalSuite, MockScript, MockTurn};
use xiaoguai_llm::{Message, ToolCallSpec};

/// Hard cap on the number of cases a single `eval/run` request may
/// execute. Anthropic's eval guidance + the CLAUDE.md context rule
/// ("agents read JSON lists token by token") both want bounded
/// responses; over-large suites belong on the CLI, not the console.
pub const MAX_CASES_PER_RUN: usize = 100;

/// Total wall-clock budget across every case in one request. Past this
/// the handler returns 504 with `gateway_timeout` so the console can
/// surface a clear "your suite is too large for the pane" error rather
/// than hanging.
pub const MAX_RUN_DURATION: Duration = Duration::from_secs(60);

/// File extension the suites loader walks for. Mirrors
/// [`xiaoguai_eval::EvalSuite::load_from_dir`].
const CASE_EXTENSION: &str = "eval.yaml";

#[derive(Debug, Error)]
pub enum EvalServiceError {
    #[error("eval backend: {0}")]
    Backend(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("suite too large: {actual} cases (max {max})")]
    SuiteTooLarge { actual: usize, max: usize },
    #[error("suite exceeded {limit_secs}s wall-clock budget")]
    SuiteTimedOut { limit_secs: u64 },
}

/// Suite list-item returned by `GET /v1/admin/eval/suites`. We keep the
/// shape narrow on purpose — directory size + case count are useful
/// affordances for the console; everything else (per-case detail) is
/// served by reading the YAML on disk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvalSuiteListItem {
    /// Suite name as the console will pass it to `/eval/run`. Matches
    /// the directory's basename (or a `.eval.yaml` filename minus the
    /// extension when suites land as single-file YAMLs).
    pub name: String,
    /// Absolute or relative path the suite resolves to on disk — useful
    /// when the operator needs to `cat` it from the shell.
    pub path: String,
    /// Number of `.eval.yaml` cases found directly under `path`.
    /// `None` when the suite is a single-file YAML.
    pub case_count: Option<usize>,
}

/// Request body for `POST /v1/admin/eval/run`. `cases_dir` overrides
/// the configured `suites_dir/<suite_name>` lookup so an operator can
/// point at an ad-hoc fixtures path during development.
#[derive(Debug, Clone, Deserialize)]
pub struct RunEvalRequest {
    pub suite_name: String,
    #[serde(default)]
    pub cases_dir: Option<String>,
}

/// Request body for `POST /v1/admin/eval/case-from-session`. Only the
/// session id is required; the source impl owns tenant scoping (PG-
/// backed source reads from `sessions` + `audit_log` joined on the
/// supplied id).
#[derive(Debug, Clone, Deserialize)]
pub struct CaseFromSessionRequest {
    pub session_id: String,
}

/// Response body for `/eval/case-from-session`. We hand the operator a
/// ready-to-paste YAML string (rather than writing the file ourselves)
/// so they review the assertions before they land on disk — same
/// philosophy as the v0.11.0 plan's "case files are authored, not
/// generated".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaseFromSessionResponse {
    pub case_yaml: String,
    pub suggested_filename: String,
    pub case_id: String,
    /// How many `tool.invoke` audit rows fed the suggested
    /// `ToolInvocationCount` / `ToolCallSequence` assertions.
    pub tool_invocation_count: usize,
}

/// Recovered tool invocation pulled from one `audit_log` row. The PG
/// adapter projects `details.tool_name` + `details.arguments` into this
/// shape; the static test impl synthesises rows from hand-built
/// fixtures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInvocationRecord {
    pub tool_name: String,
    /// Raw JSON arguments as the model emitted them. Empty `{}` when
    /// the audit row omitted them.
    pub arguments_json: String,
}

/// Session→case projection input. We keep this small on purpose: only
/// the bits required to suggest assertions belong on the boundary
/// (history + tool calls + final assistant text); richer signals (cost,
/// latency, citations) can be threaded through in v0.12.x if the
/// console grows them.
#[derive(Debug, Clone)]
pub struct SessionForCase {
    pub session_id: String,
    pub tenant_id: Option<String>,
    pub input_messages: Vec<Message>,
    pub tool_invocations: Vec<ToolInvocationRecord>,
    pub final_assistant_text: Option<String>,
}

/// Production wires a PG-backed impl in `xiaoguai-core`; the static
/// impl exercises the route + projection in unit tests without standing
/// up Postgres.
#[async_trait]
pub trait CaseFromSessionSource: Send + Sync {
    async fn load_session(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionForCase>, EvalServiceError>;
}

/// In-memory `CaseFromSessionSource` for route tests + dev mode. Holds a
/// fixed map and looks up by `session_id`.
#[derive(Debug, Default, Clone)]
pub struct StaticCaseFromSessionSource {
    pub sessions: std::collections::HashMap<String, SessionForCase>,
}

impl StaticCaseFromSessionSource {
    #[must_use]
    pub fn with_session(session: SessionForCase) -> Self {
        let mut me = Self::default();
        me.sessions.insert(session.session_id.clone(), session);
        me
    }

    #[must_use]
    pub fn add_session(mut self, session: SessionForCase) -> Self {
        self.sessions.insert(session.session_id.clone(), session);
        self
    }
}

#[async_trait]
impl CaseFromSessionSource for StaticCaseFromSessionSource {
    async fn load_session(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionForCase>, EvalServiceError> {
        Ok(self.sessions.get(session_id).cloned())
    }
}

/// Wraps the runner + suites directory + case source so handlers grab
/// one Arc out of `AppState`. Keeps trait wiring centralised.
pub struct EvalService {
    pub runner: EvalRunner,
    pub suites_dir: PathBuf,
    pub case_source: Arc<dyn CaseFromSessionSource>,
}

impl EvalService {
    #[must_use]
    pub fn new(
        runner: EvalRunner,
        suites_dir: PathBuf,
        case_source: Arc<dyn CaseFromSessionSource>,
    ) -> Self {
        Self {
            runner,
            suites_dir,
            case_source,
        }
    }

    /// List the suites discoverable under `suites_dir`. A directory
    /// holding `*.eval.yaml` files counts as one suite; a top-level
    /// `*.eval.yaml` file also counts (single-file suites stay
    /// convenient for tiny smoke checks).
    ///
    /// # Errors
    /// Returns an error if the suites directory cannot be read.
    pub fn list_suites(&self) -> Result<Vec<EvalSuiteListItem>, EvalServiceError> {
        list_suites_in(&self.suites_dir)
    }

    /// Run `suite_name` (resolved to `cases_dir` when supplied,
    /// otherwise `suites_dir/<suite_name>`). Enforces the case-count
    /// and wall-clock caps so a too-large suite returns a clear error
    /// instead of pinning the server.
    ///
    /// # Errors
    /// Returns an error if the suite is not found, too large, times out, or the runner fails.
    pub async fn run_suite(&self, req: &RunEvalRequest) -> Result<EvalReport, EvalServiceError> {
        if req.suite_name.is_empty() {
            return Err(EvalServiceError::InvalidArgument(
                "suite_name must not be empty".into(),
            ));
        }
        let dir = req
            .cases_dir
            .as_ref()
            .map_or_else(|| self.suites_dir.join(&req.suite_name), PathBuf::from);
        if !dir.exists() {
            return Err(EvalServiceError::NotFound(format!(
                "suite directory {} does not exist",
                dir.display()
            )));
        }
        let suite = EvalSuite::load_from_dir(req.suite_name.clone(), &dir)
            .map_err(|e| EvalServiceError::Backend(e.to_string()))?;
        if suite.cases.len() > MAX_CASES_PER_RUN {
            return Err(EvalServiceError::SuiteTooLarge {
                actual: suite.cases.len(),
                max: MAX_CASES_PER_RUN,
            });
        }
        match tokio::time::timeout(MAX_RUN_DURATION, self.runner.run_suite(&suite)).await {
            Ok(Ok(report)) => Ok(report),
            Ok(Err(e)) => Err(EvalServiceError::Backend(e.to_string())),
            Err(_) => Err(EvalServiceError::SuiteTimedOut {
                limit_secs: MAX_RUN_DURATION.as_secs(),
            }),
        }
    }

    /// Project a real `sessions.id` into a ready-to-edit `EvalCase`
    /// YAML string. Returns 404 when the source has no such session.
    ///
    /// # Errors
    /// Returns an error if the session is not found or YAML serialization fails.
    pub async fn case_from_session(
        &self,
        req: &CaseFromSessionRequest,
    ) -> Result<CaseFromSessionResponse, EvalServiceError> {
        if req.session_id.is_empty() {
            return Err(EvalServiceError::InvalidArgument(
                "session_id must not be empty".into(),
            ));
        }
        let session = self
            .case_source
            .load_session(&req.session_id)
            .await?
            .ok_or_else(|| EvalServiceError::NotFound(format!("session {}", req.session_id)))?;
        build_case_yaml(&session)
    }
}

/// Pure projection — kept outside `EvalService` so it's directly
/// testable without an `Arc<dyn CaseFromSessionSource>`.
///
/// # Errors
/// Returns an error if YAML serialization of the case fails.
pub fn build_case_yaml(
    session: &SessionForCase,
) -> Result<CaseFromSessionResponse, EvalServiceError> {
    let case_id = format!("from-session-{}", short_id(&session.session_id));
    let case = case_from_session_internal(&case_id, session);
    let case_yaml =
        serde_yaml::to_string(&case).map_err(|e| EvalServiceError::Backend(e.to_string()))?;
    Ok(CaseFromSessionResponse {
        case_yaml,
        suggested_filename: format!("{case_id}.{CASE_EXTENSION}"),
        case_id,
        tool_invocation_count: session.tool_invocations.len(),
    })
}

fn case_from_session_internal(case_id: &str, session: &SessionForCase) -> EvalCase {
    use xiaoguai_eval::{Assertion, ToolCallPattern};

    // Mock script: one `tool_calls` turn per recovered invocation, then a
    // final text turn replaying the assistant's last reply. The eval
    // runner needs a non-empty script and a terminal text turn or the
    // agent loop will refuse to settle.
    let mut turns: Vec<MockTurn> = Vec::new();
    for (idx, inv) in session.tool_invocations.iter().enumerate() {
        turns.push(MockTurn::tool_calls(vec![ToolCallSpec {
            id: format!("call-{}", idx + 1),
            name: inv.tool_name.clone(),
            arguments_json: inv.arguments_json.clone(),
        }]));
    }
    let final_text = session
        .final_assistant_text
        .clone()
        .unwrap_or_else(|| "<edit me: the assistant's expected final reply>".to_string());
    turns.push(MockTurn::text(final_text.clone()));

    // Assertions: pin the final reply (operator edits the substring
    // after copying) + one `ToolInvocationCount` per distinct tool, plus
    // a `ToolCallSequence` spelling out the exact order observed. These
    // are the load-bearing graders for regression-from-prod-run.
    let mut assertions = Vec::new();
    if !final_text.is_empty() {
        let snippet = first_words(&final_text, 6);
        if !snippet.is_empty() {
            assertions.push(Assertion::FinalMessageContains { text: snippet });
        }
    }
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for inv in &session.tool_invocations {
        *counts.entry(inv.tool_name.clone()).or_default() += 1;
    }
    for (tool_name, expected) in counts {
        assertions.push(Assertion::ToolInvocationCount {
            tool_name,
            expected,
        });
    }
    if !session.tool_invocations.is_empty() {
        let expected = session
            .tool_invocations
            .iter()
            .map(|inv| ToolCallPattern {
                tool_name: inv.tool_name.clone(),
                arguments_json_substring: String::new(),
            })
            .collect();
        assertions.push(Assertion::ToolCallSequence { expected });
    }

    EvalCase {
        id: case_id.into(),
        input_messages: session.input_messages.clone(),
        mock_script: Some(MockScript::new(turns)),
        assertions,
        tags: vec!["regression".into(), "from-session".into()],
    }
}

fn first_words(text: &str, n: usize) -> String {
    text.split_whitespace()
        .take(n)
        .collect::<Vec<_>>()
        .join(" ")
}

fn short_id(id: &str) -> String {
    let trimmed: String = id
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if trimmed.len() <= 12 {
        trimmed
    } else {
        trimmed.chars().take(12).collect()
    }
}

/// Enumerate suites discoverable under `dir`. Subdirectories that hold
/// at least one `*.eval.yaml` are treated as multi-case suites; loose
/// `*.eval.yaml` files at the top become single-case suites named after
/// the file stem.
///
/// # Errors
/// Returns an error if the directory cannot be read.
pub fn list_suites_in(dir: &Path) -> Result<Vec<EvalSuiteListItem>, EvalServiceError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let read = std::fs::read_dir(dir)
        .map_err(|e| EvalServiceError::Backend(format!("read_dir {}: {e}", dir.display())))?;
    let mut suites: Vec<EvalSuiteListItem> = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let count = count_case_files(&path)?;
            if count > 0 {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    suites.push(EvalSuiteListItem {
                        name: name.to_string(),
                        path: path.display().to_string(),
                        case_count: Some(count),
                    });
                }
            }
        } else if path.is_file() && is_case_file(&path) {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                // Strip the `.eval.yaml` suffix so suite_name is what
                // the CLI / endpoint expects.
                let bare = name.trim_end_matches(&format!(".{CASE_EXTENSION}"));
                suites.push(EvalSuiteListItem {
                    name: bare.to_string(),
                    path: path.display().to_string(),
                    case_count: None,
                });
            }
        }
    }
    suites.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(suites)
}

fn count_case_files(dir: &Path) -> Result<usize, EvalServiceError> {
    let read = std::fs::read_dir(dir)
        .map_err(|e| EvalServiceError::Backend(format!("read_dir {}: {e}", dir.display())))?;
    let mut n = 0;
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_file() && is_case_file(&path) {
            n += 1;
        }
    }
    Ok(n)
}

fn is_case_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(&format!(".{CASE_EXTENSION}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use xiaoguai_eval::{Assertion, DefaultEvalAgentBuilder};
    use xiaoguai_llm::Message;

    fn write_case_file(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn list_suites_includes_directory_with_cases_and_top_level_file() {
        let tmp = tempfile::tempdir().unwrap();
        // A directory suite with two cases.
        let dir_suite = tmp.path().join("regression");
        std::fs::create_dir(&dir_suite).unwrap();
        write_case_file(
            &dir_suite,
            "case_a.eval.yaml",
            "id: a\ninput_messages: []\nassertions: []\n",
        );
        write_case_file(
            &dir_suite,
            "case_b.eval.yaml",
            "id: b\ninput_messages: []\nassertions: []\n",
        );
        // A single-file suite.
        write_case_file(
            tmp.path(),
            "smoke.eval.yaml",
            "id: smoke\ninput_messages: []\nassertions: []\n",
        );
        // A non-case file should be ignored.
        write_case_file(tmp.path(), "README.md", "ignore");

        let items = list_suites_in(tmp.path()).unwrap();
        assert_eq!(items.len(), 2);
        // Alphabetical: regression < smoke.
        assert_eq!(items[0].name, "regression");
        assert_eq!(items[0].case_count, Some(2));
        assert_eq!(items[1].name, "smoke");
        assert_eq!(items[1].case_count, None);
    }

    #[test]
    fn list_suites_missing_dir_returns_empty() {
        let items = list_suites_in(Path::new("/definitely-not-a-real-path-xiaoguai")).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn list_suites_skips_directory_with_no_cases() {
        let tmp = tempfile::tempdir().unwrap();
        let empty = tmp.path().join("empty-suite");
        std::fs::create_dir(&empty).unwrap();
        write_case_file(&empty, "notes.txt", "ignore");
        let items = list_suites_in(tmp.path()).unwrap();
        assert!(items.is_empty(), "got {items:?}");
    }

    fn sample_session() -> SessionForCase {
        SessionForCase {
            session_id: "sess_abc123".into(),
            tenant_id: Some("ten".into()),
            input_messages: vec![Message::user("look up the weather in Berlin")],
            tool_invocations: vec![
                ToolInvocationRecord {
                    tool_name: "weather_lookup".into(),
                    arguments_json: "{\"city\":\"Berlin\"}".into(),
                },
                ToolInvocationRecord {
                    tool_name: "format_reply".into(),
                    arguments_json: "{}".into(),
                },
            ],
            final_assistant_text: Some("It's 18°C and sunny in Berlin.".into()),
        }
    }

    #[test]
    fn build_case_yaml_round_trips_through_eval_case_loader() {
        let resp = build_case_yaml(&sample_session()).unwrap();
        assert_eq!(resp.tool_invocation_count, 2);
        assert!(resp.suggested_filename.ends_with(".eval.yaml"));
        // The YAML must parse back into a valid EvalCase.
        let case: EvalCase = serde_yaml::from_str(&resp.case_yaml).expect("yaml round-trips");
        assert_eq!(case.id, resp.case_id);
        assert_eq!(case.tags, vec!["regression", "from-session"]);
        // Two tool turns + one final text turn.
        let script = case.mock_script.as_ref().unwrap();
        assert_eq!(script.turns.len(), 3);
        assert_eq!(script.turns[0].tool_calls[0].name, "weather_lookup");
        assert_eq!(script.turns[1].tool_calls[0].name, "format_reply");
        assert!(script.turns[2].text.contains("Berlin"));
        // Final-message + per-tool count + sequence assertions.
        assert!(case
            .assertions
            .iter()
            .any(|a| matches!(a, Assertion::FinalMessageContains { .. })));
        let tool_count = case
            .assertions
            .iter()
            .filter(|a| matches!(a, Assertion::ToolInvocationCount { .. }))
            .count();
        assert_eq!(tool_count, 2);
    }

    #[test]
    fn build_case_yaml_handles_session_with_no_tool_calls() {
        let session = SessionForCase {
            session_id: "sess_z".into(),
            tenant_id: None,
            input_messages: vec![Message::user("hi")],
            tool_invocations: Vec::new(),
            final_assistant_text: Some("hello back".into()),
        };
        let resp = build_case_yaml(&session).unwrap();
        assert_eq!(resp.tool_invocation_count, 0);
        let case: EvalCase = serde_yaml::from_str(&resp.case_yaml).unwrap();
        let script = case.mock_script.as_ref().unwrap();
        assert_eq!(script.turns.len(), 1);
        assert!(case
            .assertions
            .iter()
            .all(|a| !matches!(a, Assertion::ToolCallSequence { .. })));
    }

    fn build_service(tmp_path: &Path) -> EvalService {
        let runner = EvalRunner::new(Arc::new(DefaultEvalAgentBuilder::new(2)));
        let source = Arc::new(StaticCaseFromSessionSource::with_session(sample_session()));
        EvalService::new(runner, tmp_path.to_path_buf(), source)
    }

    #[tokio::test]
    async fn run_suite_executes_a_disk_suite() {
        let tmp = tempfile::tempdir().unwrap();
        let suite_dir = tmp.path().join("smoke");
        std::fs::create_dir(&suite_dir).unwrap();
        write_case_file(
            &suite_dir,
            "greet.eval.yaml",
            "id: greet\n\
             input_messages:\n  - role: user\n    content: hi\n\
             mock_script:\n  turns:\n    - text: hello back\n\
             assertions:\n  - kind: final_message_contains\n    text: hello\n",
        );
        let svc = build_service(tmp.path());
        let report = svc
            .run_suite(&RunEvalRequest {
                suite_name: "smoke".into(),
                cases_dir: None,
            })
            .await
            .unwrap();
        assert_eq!(report.suite, "smoke");
        assert_eq!(report.results.len(), 1);
        assert!(report.results[0].status.is_pass());
    }

    #[tokio::test]
    async fn run_suite_missing_dir_returns_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = build_service(tmp.path());
        let err = svc
            .run_suite(&RunEvalRequest {
                suite_name: "missing".into(),
                cases_dir: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, EvalServiceError::NotFound(_)), "{err:?}");
    }

    #[tokio::test]
    async fn run_suite_rejects_empty_suite_name() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = build_service(tmp.path());
        let err = svc
            .run_suite(&RunEvalRequest {
                suite_name: String::new(),
                cases_dir: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, EvalServiceError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn case_from_session_404_on_unknown_id() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = build_service(tmp.path());
        let err = svc
            .case_from_session(&CaseFromSessionRequest {
                session_id: "nope".into(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, EvalServiceError::NotFound(_)));
    }

    #[tokio::test]
    async fn case_from_session_returns_yaml_for_known_id() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = build_service(tmp.path());
        let resp = svc
            .case_from_session(&CaseFromSessionRequest {
                session_id: "sess_abc123".into(),
            })
            .await
            .unwrap();
        assert_eq!(resp.tool_invocation_count, 2);
        assert!(resp.case_yaml.contains("weather_lookup"));
    }
}
