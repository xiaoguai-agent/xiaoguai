//! Smoke-test the binary by invoking it with --mock and checking stdout.

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn cli_chat_mock_prints_canned_response() {
    let mut cmd = Command::cargo_bin("xiaoguai").expect("binary");
    cmd.args(["chat", "--prompt", "hello", "--mock"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Hello from Xiaoguai"));
}

#[test]
fn cli_help_lists_all_subcommands() {
    let mut cmd = Command::cargo_bin("xiaoguai").expect("binary");
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("chat"))
        .stdout(predicate::str::contains("provider"))
        .stdout(predicate::str::contains("mcp"));
}

#[test]
fn cli_mcp_register_help_describes_required_flags() {
    let mut cmd = Command::cargo_bin("xiaoguai").expect("binary");
    cmd.args(["mcp", "register", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("--name"))
        .stdout(predicate::str::contains("--transport"))
        .stdout(predicate::str::contains("--env-keys"));
}

#[test]
fn cli_provider_register_help_describes_required_flags() {
    let mut cmd = Command::cargo_bin("xiaoguai").expect("binary");
    cmd.args(["provider", "register", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("--name"))
        .stdout(predicate::str::contains("--endpoint"))
        .stdout(predicate::str::contains("--api-key-env"));
}
