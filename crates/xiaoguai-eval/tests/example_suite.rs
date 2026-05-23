//! End-to-end check that the shipped example suite loads + passes.
//!
//! Real on-disk suite, real YAML loader, real `MockBackend`, real
//! `ReactAgent` — if a future refactor breaks any of those this
//! test catches it before a downstream consumer does.

use std::path::PathBuf;
use std::sync::Arc;

use xiaoguai_eval::{DefaultEvalAgentBuilder, EvalRunner, EvalSuite};

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/eval/regression")
}

#[tokio::test]
async fn example_regression_suite_loads_and_passes() {
    let dir = examples_dir();
    let suite = EvalSuite::load_from_dir("regression", &dir).expect("load shipped example suite");
    assert!(
        suite.cases.len() >= 2,
        "shipped suite should have at least 2 cases; got {}",
        suite.cases.len()
    );

    let runner = EvalRunner::new(Arc::new(DefaultEvalAgentBuilder::new(4)));
    let report = runner.run_suite(&suite).await.expect("suite runs");

    assert_eq!(
        report.results.len(),
        suite.cases.len(),
        "one result per case"
    );
    let failed: Vec<_> = report
        .results
        .iter()
        .filter(|r| !r.status.is_pass())
        .collect();
    assert!(
        failed.is_empty(),
        "shipped example suite must pass; failures:\n{}",
        failed
            .iter()
            .map(|r| format!("{r:?}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!((report.pass_rate - 1.0).abs() < f32::EPSILON);
}

#[tokio::test]
async fn report_writes_json_to_disk() {
    let dir = examples_dir();
    let suite = EvalSuite::load_from_dir("regression", &dir).unwrap();
    let runner = EvalRunner::new(Arc::new(DefaultEvalAgentBuilder::new(4)));
    let report = runner.run_suite(&suite).await.unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("eval-report.json");
    report.write_json(&path).unwrap();

    let body = std::fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["suite"], "regression");
    assert!(parsed["results"].as_array().unwrap().len() >= 2);
}
