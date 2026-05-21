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
fn cli_help_lists_chat() {
    let mut cmd = Command::cargo_bin("xiaoguai").expect("binary");
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("chat"));
}
