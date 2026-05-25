//! `xiaoguai eval` — run an `EvalSuite` against the deterministic
//! `MockBackend` substrate.
//!
//! Kept thin: the CLI handler loads a directory of `.eval.yaml`
//! cases, runs them through [`xiaoguai_eval::EvalRunner`] with
//! [`xiaoguai_eval::DefaultEvalAgentBuilder`], prints the pretty
//! summary to stdout, and (optionally) writes the JSON report.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use xiaoguai_eval::{DefaultEvalAgentBuilder, EvalReport, EvalRunner, EvalSuite};

#[derive(Debug, Clone)]
pub struct EvalArgs {
    /// Suite name. Surfaces as `EvalReport.suite`. Required so CLI
    /// callers can pin a stable label without depending on a
    /// directory's basename.
    pub suite: String,
    /// Directory containing `*.eval.yaml` case files. Flat — no
    /// recursion. Defaults to `./eval/<suite>`.
    pub cases_dir: Option<PathBuf>,
    /// Optional path for the JSON report. When unset, no report
    /// file is written (stdout summary only).
    pub out: Option<PathBuf>,
    /// Hard cap on agent-loop iterations per case. Zero = use the
    /// `xiaoguai-agent` default (8).
    pub max_iterations: u32,
}

/// Run the eval suite and return the report.
///
/// # Errors
/// Returns an error if the cases directory cannot be read, if any case file
/// is malformed, or if writing the optional JSON report file fails.
pub async fn run(args: EvalArgs) -> Result<EvalReport> {
    let dir = args
        .cases_dir
        .clone()
        .unwrap_or_else(|| default_dir_for_suite(&args.suite));
    let suite = EvalSuite::load_from_dir(args.suite.clone(), &dir)
        .with_context(|| format!("load suite {:?} from {}", args.suite, dir.display()))?;

    let runner = EvalRunner::new(Arc::new(DefaultEvalAgentBuilder::new(args.max_iterations)));
    let report = runner.run_suite(&suite).await.context("run eval suite")?;

    if let Some(out) = args.out.as_ref() {
        report
            .write_json(out)
            .with_context(|| format!("write report to {}", out.display()))?;
    }
    Ok(report)
}

fn default_dir_for_suite(suite: &str) -> PathBuf {
    Path::new("eval").join(suite)
}

#[cfg(test)]
mod tests {
    use super::*;
    use xiaoguai_eval::{Assertion, EvalCase, MockScript, MockTurn};
    use xiaoguai_llm::Message;

    fn write_case(dir: &Path, name: &str, case: &EvalCase) {
        let path = dir.join(format!("{name}.eval.yaml"));
        let yaml = serde_yaml::to_string(case).unwrap();
        std::fs::write(path, yaml).unwrap();
    }

    fn passing_case(id: &str) -> EvalCase {
        EvalCase {
            id: id.into(),
            input_messages: vec![Message::user("hi")],
            mock_script: Some(MockScript::new(vec![MockTurn::text("hello back")])),
            assertions: vec![Assertion::FinalMessageContains {
                text: "hello".into(),
            }],
            tags: Vec::new(),
        }
    }

    #[tokio::test]
    async fn run_loads_dir_and_returns_report() {
        let tmp = tempfile::tempdir().unwrap();
        write_case(tmp.path(), "a", &passing_case("a"));
        write_case(tmp.path(), "b", &passing_case("b"));

        let report = run(EvalArgs {
            suite: "regression".into(),
            cases_dir: Some(tmp.path().to_path_buf()),
            out: None,
            max_iterations: 2,
        })
        .await
        .unwrap();
        assert_eq!(report.suite, "regression");
        assert_eq!(report.results.len(), 2);
        assert!(report.results.iter().all(|r| r.status.is_pass()));
    }

    #[tokio::test]
    async fn run_writes_report_when_out_provided() {
        let tmp = tempfile::tempdir().unwrap();
        write_case(tmp.path(), "only", &passing_case("only"));
        let out = tmp.path().join("report.json");

        run(EvalArgs {
            suite: "x".into(),
            cases_dir: Some(tmp.path().to_path_buf()),
            out: Some(out.clone()),
            max_iterations: 2,
        })
        .await
        .unwrap();
        assert!(out.exists());
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
        assert_eq!(parsed["suite"], "x");
    }

    #[test]
    fn default_dir_uses_suite_name() {
        let p = default_dir_for_suite("regression");
        assert_eq!(p, Path::new("eval").join("regression"));
    }

    #[tokio::test]
    async fn missing_dir_surfaces_helpful_error() {
        let err = run(EvalArgs {
            suite: "nope".into(),
            cases_dir: Some(PathBuf::from("/definitely-not-a-real-path-xyz")),
            out: None,
            max_iterations: 0,
        })
        .await
        .unwrap_err();
        let s = format!("{err:#}");
        assert!(s.contains("load suite"));
    }
}
