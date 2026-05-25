//! Integration tests for `xiaoguai tasks` subcommand.
//!
//! Uses `mockito` to stand up stub HTTP servers so no real backend is needed.
//! All tests verify the happy path, expected error payloads, and the graceful
//! 404/503 fallback message that surfaces until the v1.4 backend ships.

use assert_cmd::Command;
use predicates::prelude::*;

// ── helpers ─────────────────────────────────────────────────────────────────

/// Shared 404 fallback message text (must match `NOT_YET_AVAILABLE` const).
const NOT_YET_MSG: &str = "Tasks subsystem not yet wired (ships in v1.4). See ADR-0019.";

async fn mock_server() -> mockito::ServerGuard {
    mockito::Server::new_async().await
}

fn cli() -> Command {
    Command::cargo_bin("xiaoguai").expect("binary must be built")
}

// ── list ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_happy_path_prints_json() {
    let mut server = mock_server().await;
    let _m = server
        .mock("GET", "/v1/tasks")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"id":"t-1","title":"Fix bug","column":"triage"}]"#)
        .create_async()
        .await;

    cli()
        .args([
            "tasks",
            "--api-base",
            &server.url(),
            "list",
            "--board",
            "default",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Fix bug"));
}

#[tokio::test]
async fn list_404_prints_fallback_message() {
    let mut server = mock_server().await;
    let _m = server
        .mock("GET", "/v1/tasks")
        .match_query(mockito::Matcher::Any)
        .with_status(404)
        .with_body("")
        .create_async()
        .await;

    cli()
        .args(["tasks", "--api-base", &server.url(), "list"])
        .assert()
        .success() // CLI exits 0 even on 404 — informative message only
        .stdout(predicate::str::contains(NOT_YET_MSG));
}

#[tokio::test]
async fn list_with_column_filter() {
    let mut server = mock_server().await;
    let _m = server
        .mock("GET", "/v1/tasks")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("board".into(), "sprint".into()),
            mockito::Matcher::UrlEncoded("column".into(), "running".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"id":"t-2","title":"Deploy","column":"running"}]"#)
        .create_async()
        .await;

    cli()
        .args([
            "tasks",
            "--api-base",
            &server.url(),
            "list",
            "--board",
            "sprint",
            "--column",
            "running",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Deploy"));
}

// ── create ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_happy_path_returns_task() {
    let mut server = mock_server().await;
    let _m = server
        .mock("POST", "/v1/tasks")
        .with_status(201)
        .with_header("content-type", "application/json")
        .with_body(r#"{"id":"t-99","title":"New task","column":"triage"}"#)
        .create_async()
        .await;

    cli()
        .args([
            "tasks",
            "--api-base",
            &server.url(),
            "create",
            "--title",
            "New task",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("t-99"));
}

#[tokio::test]
async fn create_422_shows_validation_error() {
    let mut server = mock_server().await;
    let _m = server
        .mock("POST", "/v1/tasks")
        .with_status(422)
        .with_header("content-type", "application/json")
        .with_body(r#"{"detail":"title is required"}"#)
        .create_async()
        .await;

    cli()
        .args([
            "tasks",
            "--api-base",
            &server.url(),
            "create",
            "--title",
            "x",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("validation error")
                .and(predicate::str::contains("title is required")),
        );
}

// ── move ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn move_happy_path_returns_updated_task() {
    let mut server = mock_server().await;
    let _m = server
        .mock("POST", "/v1/tasks/t-1/move")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"id":"t-1","column":"running"}"#)
        .create_async()
        .await;

    cli()
        .args([
            "tasks",
            "--api-base",
            &server.url(),
            "move",
            "t-1",
            "--to",
            "running",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("running"));
}

#[tokio::test]
async fn move_invalid_column_shows_server_error() {
    let mut server = mock_server().await;
    let _m = server
        .mock("POST", "/v1/tasks/t-1/move")
        .with_status(422)
        .with_header("content-type", "application/json")
        .with_body(r#"{"detail":"unknown column 'bogus'"}"#)
        .create_async()
        .await;

    cli()
        .args([
            "tasks",
            "--api-base",
            &server.url(),
            "move",
            "t-1",
            "--to",
            "bogus",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("unknown column"));
}

// ── dispatch ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dispatch_returns_dispatched_tasks() {
    let mut server = mock_server().await;
    let _m = server
        .mock("POST", "/v1/tasks/dispatch")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"[{"id":"t-3","title":"Run tests","column":"running"}]"#)
        .create_async()
        .await;

    cli()
        .args([
            "tasks",
            "--api-base",
            &server.url(),
            "dispatch",
            "--board",
            "default",
            "--n",
            "1",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Run tests"));
}

#[tokio::test]
async fn dispatch_empty_array_prints_no_ready_cards_message() {
    let mut server = mock_server().await;
    let _m = server
        .mock("POST", "/v1/tasks/dispatch")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("[]")
        .create_async()
        .await;

    cli()
        .args([
            "tasks",
            "--api-base",
            &server.url(),
            "dispatch",
            "--board",
            "default",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("no ready cards"));
}

#[tokio::test]
async fn dispatch_503_prints_fallback_message() {
    let mut server = mock_server().await;
    let _m = server
        .mock("POST", "/v1/tasks/dispatch")
        .match_query(mockito::Matcher::Any)
        .with_status(503)
        .with_body("")
        .create_async()
        .await;

    cli()
        .args(["tasks", "--api-base", &server.url(), "dispatch"])
        .assert()
        .success()
        .stdout(predicate::str::contains(NOT_YET_MSG));
}

// ── show ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn show_happy_path_prints_task_detail() {
    let mut server = mock_server().await;
    let _m = server
        .mock("GET", "/v1/tasks/t-42")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"id":"t-42","title":"Big refactor","history":[]}"#)
        .create_async()
        .await;

    cli()
        .args(["tasks", "--api-base", &server.url(), "show", "t-42"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Big refactor"));
}

// ── help surface ─────────────────────────────────────────────────────────────

#[test]
fn tasks_help_lists_all_subcommands() {
    cli()
        .args(["tasks", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("create"))
        .stdout(predicate::str::contains("move"))
        .stdout(predicate::str::contains("claim"))
        .stdout(predicate::str::contains("complete"))
        .stdout(predicate::str::contains("block"))
        .stdout(predicate::str::contains("dispatch"))
        .stdout(predicate::str::contains("show"));
}

#[test]
fn tasks_appears_in_top_level_help() {
    cli()
        .args(["--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("tasks"));
}
