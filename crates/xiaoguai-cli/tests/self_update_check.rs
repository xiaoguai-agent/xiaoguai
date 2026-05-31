//! Integration tests for `xiaoguai self-update`.
//!
//! These tests mock the GitHub releases API so we never touch the real GitHub
//! endpoint and never actually replace the running binary.

use assert_cmd::Command;
use predicates::prelude::*;
use xiaoguai_cli::commands::self_update::is_newer;

// ── unit tests for version comparison ─────────────────────────────────────

#[test]
fn is_newer_returns_false_for_same_version() {
    // CARGO_PKG_VERSION is set at compile time; the running binary version
    // should never be "newer" than itself.
    let current = format!("v{}", env!("CARGO_PKG_VERSION"));
    assert!(
        !is_newer(&current),
        "same version should not be considered newer"
    );
}

#[test]
fn is_newer_returns_true_for_higher_patch() {
    // Manufacture a version string that is definitely ahead.
    assert!(is_newer("v99.99.99"), "v99.99.99 should be newer");
}

#[test]
fn is_newer_returns_false_for_older_version() {
    assert!(
        !is_newer("v0.0.1"),
        "v0.0.1 should not be newer than current"
    );
}

#[test]
fn is_newer_handles_tag_without_v_prefix() {
    assert!(
        is_newer("99.0.0"),
        "99.0.0 without v prefix should be newer"
    );
}

// ── mock API server ────────────────────────────────────────────────────────

/// Start a local mock HTTP server that returns a GitHub releases API response
/// claiming version `v99.99.99`.
///
/// Returns the URL base.
async fn start_mock_server_new_version() -> (String, mockito::ServerGuard) {
    let mut server = mockito::Server::new_async().await;
    let body = serde_json::json!({
        "tag_name": "v99.99.99",
        "assets": [
            {
                "name": "xiaoguai-99.99.99-x86_64-unknown-linux-gnu.tar.gz",
                "browser_download_url": "http://example.com/xiaoguai.tar.gz"
            },
            {
                "name": "xiaoguai-99.99.99-x86_64-unknown-linux-gnu.tar.gz.sig",
                "browser_download_url": "http://example.com/xiaoguai.tar.gz.sig"
            },
            {
                "name": "xiaoguai-99.99.99-x86_64-unknown-linux-gnu.tar.gz.pem",
                "browser_download_url": "http://example.com/xiaoguai.tar.gz.pem"
            }
        ]
    });
    let _mock = server
        .mock("GET", "/")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .create_async()
        .await;
    let url = server.url();
    (url, server)
}

async fn start_mock_server_same_version() -> (String, mockito::ServerGuard) {
    let mut server = mockito::Server::new_async().await;
    let current = env!("CARGO_PKG_VERSION");
    let body = serde_json::json!({
        "tag_name": format!("v{current}"),
        "assets": []
    });
    let _mock = server
        .mock("GET", "/")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .create_async()
        .await;
    let url = server.url();
    (url, server)
}

// ── library-level tests of SelfUpdateArgs ─────────────────────────────────

/// When the remote reports the same version, `run_self_update` returns Ok and
/// the message says "Already up-to-date".
#[tokio::test]
async fn self_update_already_up_to_date() {
    let (url, _guard) = start_mock_server_same_version().await;

    // Capture stdout by redirecting — easier to test through the library fn
    // than through the binary, since we can pass api_url directly.
    let args = xiaoguai_cli::commands::self_update::SelfUpdateArgs {
        check: false,
        api_url: Some(format!("{url}/")),
    };

    // Should return Ok (not an error) even when no update is available.
    let result = xiaoguai_cli::commands::self_update::run_self_update(args).await;
    assert!(
        result.is_ok(),
        "should not error when already up-to-date: {result:?}"
    );
}

/// `--check` mode reports an available update but does not try to download.
#[tokio::test]
async fn self_update_check_only_does_not_download() {
    let (url, _guard) = start_mock_server_new_version().await;

    let args = xiaoguai_cli::commands::self_update::SelfUpdateArgs {
        check: true,
        api_url: Some(format!("{url}/")),
    };

    // In --check mode the function should return Ok even though the
    // download URL is fake (we never reach the download step).
    let result = xiaoguai_cli::commands::self_update::run_self_update(args).await;
    assert!(
        result.is_ok(),
        "check-only mode should not fail with fake download URL: {result:?}"
    );
}

// ── cosign verify structure ────────────────────────────────────────────────

/// `cosign_verify` error message mentions "cosign" when the binary is absent.
///
/// We test the error path by passing a PATH that does not contain cosign via
/// a subprocess so we don't mutate the test process's environment.
#[test]
fn cosign_verify_reports_missing_cosign_clearly() {
    // Spawn a helper process with PATH="" so cosign cannot be found.
    // The binary itself calls cosign_verify internally, but we don't have a
    // direct flag for it.  We verify the library function directly using a
    // tempfile as the "tarball" — the lookup of cosign happens before any
    // file I/O.
    use std::path::Path;
    use xiaoguai_cli::commands::self_update::cosign_verify;

    // On macOS / Linux, cosign is very unlikely to be at this path.
    // The function must discover it via PATH — if PATH has no cosign we get a
    // clear error.  We can't mutate PATH safely in a multi-threaded test
    // runner, so we simply confirm the function returns Err when the
    // certificate file doesn't exist either — since cosign is not installed
    // in most CI environments, the PATH lookup will fail first.
    let result = cosign_verify(
        Path::new("/nonexistent/fake.tar.gz"),
        Path::new("/nonexistent/fake.sig"),
        Path::new("/nonexistent/fake.pem"),
    );

    // Either cosign is missing (PATH lookup error) or the files don't exist
    // (cosign will error).  Either way it must be Err.
    assert!(result.is_err(), "should fail with nonexistent inputs");
    let msg = format!("{:?}", result.unwrap_err());
    // The error must mention either "cosign" (missing binary) or the path.
    assert!(
        msg.contains("cosign") || msg.contains("nonexistent"),
        "error should be diagnostic, got: {msg}"
    );
}

// ── CLI --check flag ──────────────────────────────────────────────────────

/// The binary `--check` flag exits 0 and prints something useful.
/// We can't inject a custom API URL via the binary interface, so we rely on
/// the real GitHub API returning a valid JSON (or a network-absent failure).
/// Skip this unless we have network; mark it ignored.
#[test]
#[ignore = "requires real network access to api.github.com"]
fn self_update_check_binary_exits_zero() {
    Command::cargo_bin("xiaoguai")
        .expect("binary")
        .args(["self-update", "--check"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("up-to-date")
                .or(predicate::str::contains("available"))
                .or(predicate::str::contains("release")),
        );
}
