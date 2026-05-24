//! Integration tests for `xiaoguai completions`.

use assert_cmd::Command;
use predicates::prelude::*;

/// Bash completion script is non-empty and contains key command names.
#[test]
fn bash_completion_is_non_empty_and_contains_commands() {
    let mut cmd = Command::cargo_bin("xiaoguai").expect("binary");
    cmd.args(["completions", "bash"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::is_empty().not())
        .stdout(predicate::str::contains("chat"))
        .stdout(predicate::str::contains("provider"))
        .stdout(predicate::str::contains("mcp"));
}

/// Zsh completion script is non-empty.
#[test]
fn zsh_completion_is_non_empty() {
    let mut cmd = Command::cargo_bin("xiaoguai").expect("binary");
    cmd.args(["completions", "zsh"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::is_empty().not());
}

/// Fish completion script is non-empty and references the binary name.
#[test]
fn fish_completion_references_binary_name() {
    let mut cmd = Command::cargo_bin("xiaoguai").expect("binary");
    cmd.args(["completions", "fish"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("xiaoguai"));
}

/// `PowerShell` completion script is non-empty.
#[test]
fn pwsh_completion_is_non_empty() {
    let mut cmd = Command::cargo_bin("xiaoguai").expect("binary");
    cmd.args(["completions", "pwsh"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::is_empty().not());
}

/// Elvish completion script is non-empty.
#[test]
fn elvish_completion_is_non_empty() {
    let mut cmd = Command::cargo_bin("xiaoguai").expect("binary");
    cmd.args(["completions", "elvish"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::is_empty().not());
}

/// Invalid shell name fails with a helpful error.
#[test]
fn invalid_shell_fails() {
    let mut cmd = Command::cargo_bin("xiaoguai").expect("binary");
    cmd.args(["completions", "tcsh"]);
    cmd.assert().failure();
}
