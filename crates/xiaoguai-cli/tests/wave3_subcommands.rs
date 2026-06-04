//! Integration tests for wave-3 CLI subcommands: hotl, outcomes, skills, watch, anomaly.
//!
//! Uses a `mockito` stub HTTP server to verify that each subcommand constructs
//! the correct HTTP request and handles the response (including the informative
//! 503 error path). No real Postgres or API server needed.

use mockito::{Matcher, Server};
use xiaoguai_cli::commands::{anomaly, hotl, outcomes, skills, watch};

// ---------------------------------------------------------------------------
// hotl
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hotl_policy_create_posts_to_hotl_policies() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("POST", "/v1/hotl/policies")
        .with_status(201)
        .with_header("content-type", "application/json")
        .with_body(r#"{"id":"pol-1","scope":"llm_call","window_seconds":3600,"max_count":100,"max_usd":null,"escalate_to":null}"#)
        .create_async()
        .await;

    let result = hotl::policy_create(hotl::PolicyCreateArgs {
        api_base: server.url(),
        scope: "llm_call".into(),
        window_secs: 3600,
        max_count: Some(100),
        max_usd: None,
        escalate_to: None,
    })
    .await
    .expect("policy_create ok");

    assert_eq!(result["id"], "pol-1");
    assert_eq!(result["scope"], "llm_call");
}

#[tokio::test]
async fn hotl_policy_create_requires_at_least_one_limit() {
    let server = Server::new_async().await;
    let err = hotl::policy_create(hotl::PolicyCreateArgs {
        api_base: server.url(),
        scope: "llm_call".into(),
        window_secs: 3600,
        max_count: None,
        max_usd: None,
        escalate_to: None,
    })
    .await
    .expect_err("should fail");
    assert!(
        err.to_string().contains("max-count") || err.to_string().contains("max_count"),
        "got: {err}"
    );
}

#[tokio::test]
async fn hotl_policy_list_queries() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", Matcher::Regex(r"/v1/hotl/policies".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"id":"pol-1","scope":"llm_call","window_seconds":3600}]"#)
        .create_async()
        .await;

    let rows = hotl::policy_list(hotl::PolicyListArgs {
        api_base: server.url(),
        scope: None,
    })
    .await
    .expect("policy_list ok");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], "pol-1");
}

#[tokio::test]
async fn hotl_policy_get_fetches_by_id() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/v1/hotl/policies/pol-99")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"id":"pol-99","scope":"webhook_invoke"}"#)
        .create_async()
        .await;

    let v = hotl::policy_get(hotl::PolicyGetArgs {
        api_base: server.url(),
        id: "pol-99".into(),
    })
    .await
    .expect("policy_get ok");

    assert_eq!(v["id"], "pol-99");
}

#[tokio::test]
async fn hotl_policy_delete_sends_delete() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("DELETE", "/v1/hotl/policies/pol-del")
        .with_status(204)
        .create_async()
        .await;

    hotl::policy_delete(hotl::PolicyDeleteArgs {
        api_base: server.url(),
        id: "pol-del".into(),
    })
    .await
    .expect("policy_delete ok");
}

#[tokio::test]
async fn hotl_check_returns_verdict() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("POST", "/v1/hotl/check")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"verdict":"Allow","reason":null}"#)
        .create_async()
        .await;

    let resp = hotl::check(hotl::CheckArgs {
        api_base: server.url(),
        scope: "llm_call".into(),
        amount: 1.0,
    })
    .await
    .expect("check ok");

    assert_eq!(resp.verdict, "Allow");
}

#[tokio::test]
async fn hotl_503_gives_pg_bridge_message() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", Matcher::Regex(r"/v1/hotl/policies.*".into()))
        .with_status(503)
        .create_async()
        .await;

    let err = hotl::policy_list(hotl::PolicyListArgs {
        api_base: server.url(),
        scope: None,
    })
    .await
    .expect_err("should fail");

    assert!(
        err.to_string().contains("503") || err.to_string().contains("Pg bridge"),
        "got: {err}"
    );
}

#[must_use]
fn hotl_table_not_empty(rows: &[serde_json::Value]) -> bool {
    !hotl::format_policy_table(rows).is_empty()
}

#[test]
fn hotl_format_policy_table_renders_header() {
    let rows = vec![serde_json::json!({
        "id": "p1",
        "scope": "llm_call",
        "window_seconds": 3600_u64,
        "max_count": 100_u64,
        "max_usd": null,
        "escalate_to": null
    })];
    let table = hotl::format_policy_table(&rows);
    assert!(table.contains("ID"));
    assert!(table.contains("llm_call"));
    assert!(hotl_table_not_empty(&rows));
}

// ---------------------------------------------------------------------------
// outcomes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn outcomes_record_posts_to_outcomes() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("POST", "/v1/outcomes")
        .with_status(201)
        .with_header("content-type", "application/json")
        .with_body(r#"{"ok":true}"#)
        .create_async()
        .await;

    let v = outcomes::record(outcomes::RecordArgs {
        api_base: server.url(),
        agent_name: "sales-assist".into(),
        kind: "revenue_usd".into(),
        value: 1200.0,
        session_id: None,
        unit: None,
        description: None,
    })
    .await
    .expect("record ok");

    assert_eq!(v["ok"], true);
}

#[tokio::test]
async fn outcomes_record_rejects_negative_value() {
    let server = Server::new_async().await;
    let err = outcomes::record(outcomes::RecordArgs {
        api_base: server.url(),
        agent_name: "bot".into(),
        kind: "revenue_usd".into(),
        value: -1.0,
        session_id: None,
        unit: None,
        description: None,
    })
    .await
    .expect_err("should fail");
    assert!(err.to_string().contains("non-negative"), "got: {err}");
}

#[tokio::test]
async fn outcomes_record_rejects_unknown_kind() {
    let server = Server::new_async().await;
    let err = outcomes::record(outcomes::RecordArgs {
        api_base: server.url(),
        agent_name: "bot".into(),
        kind: "magic_metric".into(),
        value: 1.0,
        session_id: None,
        unit: None,
        description: None,
    })
    .await
    .expect_err("should fail");
    assert!(err.to_string().contains("kind"), "got: {err}");
}

#[tokio::test]
async fn outcomes_list_queries_tenant() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", Matcher::Regex(r"/v1/outcomes\?".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"recorded_at":"2026-05-25T09:00:00Z","agent_name":"bot","kind":"hours_saved","value":3.0,"session_id":null}]"#)
        .create_async()
        .await;

    let rows = outcomes::list(outcomes::ListArgs {
        api_base: server.url(),
        range: "7d".into(),
        kind: None,
        limit: 100,
    })
    .await
    .expect("list ok");

    assert_eq!(rows.len(), 1);
}

#[tokio::test]
async fn outcomes_summary_queries_summary_endpoint() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", Matcher::Regex(r"/v1/outcomes/summary\?".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"kind":"revenue_usd","total":18400.0,"count":12,"avg":1533.33}]"#)
        .create_async()
        .await;

    let rows = outcomes::summary(outcomes::SummaryArgs {
        api_base: server.url(),
        range: "30d".into(),
    })
    .await
    .expect("summary ok");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["kind"], "revenue_usd");
}

#[tokio::test]
async fn outcomes_timeseries_queries_timeseries_endpoint() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", Matcher::Regex(r"/v1/outcomes/timeseries\?".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"days":[{"date":"2026-05-25","revenue_usd":9000.0}]}"#)
        .create_async()
        .await;

    let v = outcomes::timeseries(outcomes::TimeseriesArgs {
        api_base: server.url(),
        range: "7d".into(),
        kind: Some("revenue_usd".into()),
    })
    .await
    .expect("timeseries ok");

    assert!(v["days"].is_array());
}

#[tokio::test]
async fn outcomes_503_gives_pg_bridge_message() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("POST", "/v1/outcomes")
        .with_status(503)
        .create_async()
        .await;

    let err = outcomes::record(outcomes::RecordArgs {
        api_base: server.url(),
        agent_name: "bot".into(),
        kind: "hours_saved".into(),
        value: 1.0,
        session_id: None,
        unit: None,
        description: None,
    })
    .await
    .expect_err("should fail");

    assert!(
        err.to_string().contains("503") || err.to_string().contains("Pg bridge"),
        "got: {err}"
    );
}

#[test]
fn outcomes_format_list_table_renders_header() {
    let rows = vec![serde_json::json!({
        "recorded_at": "2026-05-25T09:00:00Z",
        "agent_name": "bot",
        "kind": "hours_saved",
        "value": 3.0_f64,
        "session_id": null
    })];
    let table = outcomes::format_list_table(&rows);
    assert!(table.contains("RECORDED_AT"));
    assert!(table.contains("hours_saved"));
}

#[test]
fn outcomes_format_summary_table_renders_header() {
    let rows = vec![serde_json::json!({
        "kind": "revenue_usd",
        "total": 18400.0_f64,
        "count": 12_u64,
        "avg": 1533.33_f64
    })];
    let table = outcomes::format_summary_table(&rows);
    assert!(table.contains("KIND"));
    assert!(table.contains("revenue_usd"));
}

// ---------------------------------------------------------------------------
// skills
// ---------------------------------------------------------------------------

#[tokio::test]
async fn skills_list_catalog_hits_catalog_endpoint() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/v1/skills/catalog")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"slug":"ar-collections","name":"AR Collections Assistant","version":"1.0.0","category":"finance","description":"Automates AR workflows"}]"#)
        .create_async()
        .await;

    let rows = skills::list(skills::ListArgs {
        api_base: server.url(),
        category: None,
        installed: false,
    })
    .await
    .expect("list ok");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["slug"], "ar-collections");
}

#[tokio::test]
async fn skills_list_installed_hits_installed_endpoint() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", Matcher::Regex(r"/v1/skills/installed".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"id":"inst-1","pack_slug":"ar-collections","version":"1.0.0","installed_at":"2026-05-20T10:00:00Z"}]"#)
        .create_async()
        .await;

    let rows = skills::list(skills::ListArgs {
        api_base: server.url(),
        category: None,
        installed: true,
    })
    .await
    .expect("list installed ok");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], "inst-1");
}

#[tokio::test]
async fn skills_install_posts_to_install_endpoint() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("POST", "/v1/skills/install")
        .with_status(201)
        .with_header("content-type", "application/json")
        .with_body(r#"{"id":"inst-2","pack_slug":"ar-collections","version":"1.0.0","installed_at":"2026-05-25T12:00:00Z"}"#)
        .create_async()
        .await;

    let v = skills::install(skills::InstallArgs {
        api_base: server.url(),
        pack: "ar-collections".into(),
        config: None,
    })
    .await
    .expect("install ok");

    assert_eq!(v["id"], "inst-2");
}

#[tokio::test]
async fn skills_install_with_invalid_config_json_fails_early() {
    let server = Server::new_async().await;
    let err = skills::install(skills::InstallArgs {
        api_base: server.url(),
        pack: "ar-collections".into(),
        config: Some("{not valid json".into()),
    })
    .await
    .expect_err("should fail");
    assert!(err.to_string().contains("JSON"), "got: {err}");
}

#[test]
fn skills_install_from_file_not_implemented_returns_error() {
    let err = skills::install_from_file_not_implemented().expect_err("should fail");
    assert!(
        err.to_string().contains("v1.3") || err.to_string().contains("not implemented"),
        "got: {err}"
    );
}

#[tokio::test]
async fn skills_uninstall_sends_delete() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("DELETE", "/v1/skills/install/inst-del")
        .with_status(204)
        .create_async()
        .await;

    skills::uninstall(skills::UninstallArgs {
        api_base: server.url(),
        id: "inst-del".into(),
    })
    .await
    .expect("uninstall ok");
}

#[tokio::test]
async fn skills_503_gives_pg_bridge_message() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/v1/skills/catalog")
        .with_status(503)
        .create_async()
        .await;

    let err = skills::list(skills::ListArgs {
        api_base: server.url(),
        category: None,
        installed: false,
    })
    .await
    .expect_err("should fail");

    assert!(
        err.to_string().contains("503") || err.to_string().contains("Pg bridge"),
        "got: {err}"
    );
}

#[test]
fn skills_format_catalog_table_renders_header() {
    let rows = vec![serde_json::json!({
        "slug": "ar-collections",
        "name": "AR Collections Assistant",
        "version": "1.0.0",
        "category": "finance",
        "description": "Automates AR workflows"
    })];
    let table = skills::format_catalog_table(&rows);
    assert!(table.contains("SLUG"));
    assert!(table.contains("ar-collections"));
}

#[test]
fn skills_format_installed_table_renders_header() {
    let rows = vec![serde_json::json!({
        "id": "inst-1",
        "pack_slug": "ar-collections",
        "version": "1.0.0",
        "installed_at": "2026-05-20T10:00:00Z"
    })];
    let table = skills::format_installed_table(&rows);
    assert!(table.contains("ID"));
    assert!(table.contains("ar-collections"));
}

// ---------------------------------------------------------------------------
// watch
// ---------------------------------------------------------------------------

#[tokio::test]
async fn watch_list_hits_watch_endpoint() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/v1/watch")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"id":"ar-aging","schedule":"interval 86400s","source":"sql","action":"notify","status":"active"}]"#)
        .create_async()
        .await;

    let rows = watch::list(watch::ListArgs {
        api_base: server.url(),
    })
    .await
    .expect("list ok");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], "ar-aging");
}

#[tokio::test]
async fn watch_stop_sends_delete() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("DELETE", "/v1/watch/ar-aging")
        .with_status(204)
        .create_async()
        .await;

    watch::stop(watch::StopArgs {
        api_base: server.url(),
        id: "ar-aging".into(),
    })
    .await
    .expect("stop ok");
}

#[tokio::test]
async fn watch_test_posts_to_test_endpoint() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("POST", "/v1/watch/ar-aging/test")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"matched":2,"rows":[{"customer":"Contoso","dso":72}]}"#)
        .create_async()
        .await;

    let v = watch::test(watch::TestArgs {
        api_base: server.url(),
        id: "ar-aging".into(),
    })
    .await
    .expect("test ok");

    assert_eq!(v["matched"], 2);
}

#[tokio::test]
async fn watch_503_gives_pg_bridge_message() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/v1/watch")
        .with_status(503)
        .create_async()
        .await;

    let err = watch::list(watch::ListArgs {
        api_base: server.url(),
    })
    .await
    .expect_err("should fail");

    assert!(
        err.to_string().contains("503") || err.to_string().contains("Pg bridge"),
        "got: {err}"
    );
}

#[test]
fn watch_format_list_table_renders_header() {
    let rows = vec![serde_json::json!({
        "id": "ar-aging",
        "schedule": "interval 86400s",
        "source": "sql",
        "action": "notify",
        "status": "active"
    })];
    let table = watch::format_list_table(&rows);
    assert!(table.contains("ID"));
    assert!(table.contains("ar-aging"));
}

// ---------------------------------------------------------------------------
// anomaly
// ---------------------------------------------------------------------------

#[tokio::test]
async fn anomaly_run_posts_spec_to_run_endpoint() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("POST", "/v1/anomaly/run")
        .with_status(201)
        .with_header("content-type", "application/json")
        .with_body(r#"{"registered":"orders","detector":"z_score"}"#)
        .create_async()
        .await;

    // Write a minimal spec file.
    let dir = tempfile::tempdir().expect("tmpdir");
    let spec_path = dir.path().join("spec.yaml");
    std::fs::write(
        &spec_path,
        "id: orders\nkpi_query: SELECT 1\nwindow:\n  hours: 1\n",
    )
    .expect("write spec");

    let v = anomaly::run(anomaly::RunArgs {
        api_base: server.url(),
        file: spec_path,
    })
    .await
    .expect("run ok");

    assert_eq!(v["registered"], "orders");
}

#[tokio::test]
async fn anomaly_run_missing_file_errors_clearly() {
    let server = Server::new_async().await;
    let err = anomaly::run(anomaly::RunArgs {
        api_base: server.url(),
        file: std::path::PathBuf::from("/nonexistent/path/spec.yaml"),
    })
    .await
    .expect_err("should fail");
    assert!(
        err.to_string().contains("spec") || err.to_string().contains("nonexistent"),
        "got: {err}"
    );
}

#[tokio::test]
async fn anomaly_backtest_posts_to_test_endpoint() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("POST", "/v1/anomaly/test")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"anomalies":[{"ts":"2026-05-20T00:00:00Z","value":312.0,"mean":1018.3,"std":48.7,"score":-14.5,"description":"outlier"}],"summary":"1 anomaly in 60 observations"}"#)
        .create_async()
        .await;

    let dir = tempfile::tempdir().expect("tmpdir");
    let spec_path = dir.path().join("spec.yaml");
    let csv_path = dir.path().join("data.csv");
    std::fs::write(
        &spec_path,
        "id: orders\nkpi_query: SELECT 1\nwindow:\n  hours: 1\n",
    )
    .expect("write spec");
    std::fs::write(&csv_path, "ts,value\n2026-05-20T00:00:00Z,312\n").expect("write csv");

    let v = anomaly::backtest(anomaly::BacktestArgs {
        api_base: server.url(),
        file: spec_path,
        data: csv_path,
        ts_col: "ts".into(),
        val_col: "value".into(),
    })
    .await
    .expect("backtest ok");

    assert!(v["anomalies"].is_array());
}

#[tokio::test]
async fn anomaly_503_gives_pg_bridge_message() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("POST", "/v1/anomaly/run")
        .with_status(503)
        .create_async()
        .await;

    let dir = tempfile::tempdir().expect("tmpdir");
    let spec_path = dir.path().join("spec.yaml");
    std::fs::write(
        &spec_path,
        "id: x\nkpi_query: SELECT 1\nwindow:\n  hours: 1\n",
    )
    .expect("write");

    let err = anomaly::run(anomaly::RunArgs {
        api_base: server.url(),
        file: spec_path,
    })
    .await
    .expect_err("should fail");

    assert!(
        err.to_string().contains("503") || err.to_string().contains("Pg bridge"),
        "got: {err}"
    );
}

#[test]
fn anomaly_format_backtest_table_renders_anomaly_rows() {
    let result = serde_json::json!({
        "anomalies": [
            {"ts": "2026-05-20T00:00:00Z", "value": 312.0_f64, "mean": 1018.3_f64, "std": 48.7_f64, "score": -14.5_f64, "description": "outlier"}
        ],
        "summary": "1 anomaly in 60 observations"
    });
    let table = anomaly::format_backtest_table(&result);
    assert!(table.contains("ANOMALY"));
    assert!(table.contains('*'));
    assert!(table.contains("summary"));
}

// ---------------------------------------------------------------------------
// Smoke test — help output contains all wave-3 subcommands
// ---------------------------------------------------------------------------

#[test]
fn cli_help_lists_wave3_subcommands() {
    use assert_cmd::Command;
    use predicates::prelude::*;

    let mut cmd = Command::cargo_bin("xiaoguai").expect("binary");
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("hotl"))
        .stdout(predicate::str::contains("outcomes"))
        .stdout(predicate::str::contains("skills"))
        .stdout(predicate::str::contains("watch"))
        .stdout(predicate::str::contains("anomaly"));
}

#[test]
fn hotl_policy_create_help_lists_required_flags() {
    use assert_cmd::Command;
    use predicates::prelude::*;

    let mut cmd = Command::cargo_bin("xiaoguai").expect("binary");
    cmd.args(["hotl", "policy", "create", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("--scope"))
        .stdout(predicate::str::contains("--window-secs"));
}

#[test]
fn outcomes_record_help_lists_required_flags() {
    use assert_cmd::Command;
    use predicates::prelude::*;

    let mut cmd = Command::cargo_bin("xiaoguai").expect("binary");
    cmd.args(["outcomes", "record", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("--kind"))
        .stdout(predicate::str::contains("--value"));
}

#[test]
fn skills_install_help_lists_required_flags() {
    use assert_cmd::Command;
    use predicates::prelude::*;

    let mut cmd = Command::cargo_bin("xiaoguai").expect("binary");
    cmd.args(["skills", "install", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("--pack"));
}

#[test]
fn watch_start_help_lists_required_flags() {
    use assert_cmd::Command;
    use predicates::prelude::*;

    let mut cmd = Command::cargo_bin("xiaoguai").expect("binary");
    cmd.args(["watch", "start", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("--file"));
}

#[test]
fn anomaly_run_help_lists_required_flags() {
    use assert_cmd::Command;
    use predicates::prelude::*;

    let mut cmd = Command::cargo_bin("xiaoguai").expect("binary");
    cmd.args(["anomaly", "run", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("--file"));
}
