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
fn cli_help_lists_chat_and_provider() {
    let mut cmd = Command::cargo_bin("xiaoguai").expect("binary");
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("chat"))
        .stdout(predicate::str::contains("provider"));
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
