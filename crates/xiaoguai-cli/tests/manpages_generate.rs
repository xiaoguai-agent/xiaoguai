//! Integration tests for `xiaoguai manpages`.

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

/// `xiaoguai manpages <dir>` writes a `xiaoguai.1` file containing a NAME
/// section (standard man page format).
#[test]
fn generates_main_man_page_with_name_section() {
    let dir = TempDir::new().expect("temp dir");
    let outdir = dir.path().to_str().unwrap();

    Command::cargo_bin("xiaoguai")
        .expect("binary")
        .args(["manpages", outdir])
        .assert()
        .success()
        .stdout(predicate::str::contains("xiaoguai.1"));

    let man_file = dir.path().join("xiaoguai.1");
    assert!(man_file.exists(), "xiaoguai.1 was not created");

    let contents = std::fs::read_to_string(&man_file).expect("read man file");
    assert!(
        contents.contains("NAME") || contents.contains(".SH"),
        "man page should contain a NAME section, got:\n{contents}"
    );
}

/// Subcommand man pages are also generated.
#[test]
fn generates_subcommand_man_pages() {
    let dir = TempDir::new().expect("temp dir");
    let outdir = dir.path().to_str().unwrap();

    Command::cargo_bin("xiaoguai")
        .expect("binary")
        .args(["manpages", outdir])
        .assert()
        .success();

    // At minimum the chat subcommand page should exist.
    let chat_man = dir.path().join("xiaoguai-chat.1");
    assert!(
        chat_man.exists(),
        "xiaoguai-chat.1 was not created. Files in dir: {:?}",
        std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>()
    );
}

/// Output directory is created automatically when it does not exist.
#[test]
fn creates_output_directory_if_absent() {
    let dir = TempDir::new().expect("temp dir");
    let outdir = dir.path().join("does-not-exist-yet");
    let outdir_str = outdir.to_str().unwrap();

    Command::cargo_bin("xiaoguai")
        .expect("binary")
        .args(["manpages", outdir_str])
        .assert()
        .success();

    assert!(outdir.is_dir(), "output directory should have been created");
}
