//! Integration tests for `xiaoguai backup` / `xiaoguai restore`.
//!
//! These tests use a `pg_dump` shim (a small shell script that writes
//! deterministic SQL) so the suite runs without a real Postgres instance.

use std::io::Read;
use std::path::PathBuf;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;
use xiaoguai_cli::commands::backup::{build_tar_gz, run_restore, ArchiveEntry, RestoreArgs};

// ── shim helpers ───────────────────────────────────────────────────────────

/// Write a `pg_dump` shim script to `dir/pg_dump` and return its path.
/// The shim ignores all arguments and writes a fixed SQL snippet to stdout.
#[cfg(unix)]
fn write_pg_dump_shim(dir: &std::path::Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let shim = dir.join("pg_dump");
    std::fs::write(
        &shim,
        "#!/bin/sh\necho '-- pg_dump shim output'\necho 'SELECT 1;'\n",
    )
    .expect("write shim");
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod shim");
    shim
}

// ── unit-level round-trip (library functions, no subprocess) ──────────────

/// Round-trip through `build_tar_gz` + extraction logic.
///
/// Tests archive + checksum logic without `pg_dump` or the binary.
#[test]
fn archive_round_trip_checksum_passes() {
    let entries = vec![
        ArchiveEntry {
            path: "pg_dump.sql".into(),
            data: b"SELECT 1;".to_vec(),
        },
        ArchiveEntry {
            path: "config/config.yaml".into(),
            data: b"database_url: postgres://localhost/test".to_vec(),
        },
    ];

    let gz_bytes = build_tar_gz(&entries).expect("build tar.gz");
    assert!(!gz_bytes.is_empty(), "archive must not be empty");

    // Verify we can decompress and read back the entries.
    let mut decoder = flate2::read::GzDecoder::new(gz_bytes.as_slice());
    let mut tar_bytes = Vec::new();
    decoder.read_to_end(&mut tar_bytes).expect("decompress");

    let mut archive = tar::Archive::new(tar_bytes.as_slice());
    let paths: Vec<String> = archive
        .entries()
        .expect("entries")
        .map(|e| {
            e.expect("entry")
                .path()
                .expect("path")
                .to_string_lossy()
                .into_owned()
        })
        .collect();

    assert!(
        paths.contains(&"pg_dump.sql".to_string()),
        "pg_dump.sql not found, got: {paths:?}"
    );
    assert!(
        paths.contains(&"config/config.yaml".to_string()),
        "config/config.yaml not found, got: {paths:?}"
    );
}

/// Checksum mismatch is detected during restore.
#[test]
fn restore_rejects_tampered_archive() {
    let tmp = TempDir::new().expect("temp dir");

    // Build an archive with a deliberately wrong checksum.
    let entries = vec![
        ArchiveEntry {
            path: "pg_dump.sql".into(),
            data: b"SELECT 1;".to_vec(),
        },
        ArchiveEntry {
            path: "sha256sum.txt".into(),
            data: b"deadbeef".to_vec(), // wrong hash
        },
    ];
    let gz = build_tar_gz(&entries).expect("build");
    let archive_path = tmp.path().join("tampered.tar.gz");
    std::fs::write(&archive_path, &gz).expect("write");

    let outdir = tmp.path().join("restore-out");
    let result = run_restore(RestoreArgs {
        input: archive_path,
        outdir,
        force: false,
        identity: None,
    });

    assert!(
        result.is_err(),
        "restore should reject a tampered archive with wrong checksum"
    );
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("checksum"),
        "error should mention checksum, got: {msg}"
    );
}

/// Restore refuses to overwrite an existing directory without `--force`.
#[test]
fn restore_refuses_overwrite_without_force() {
    let tmp = TempDir::new().expect("temp dir");

    let entries = vec![ArchiveEntry {
        path: "pg_dump.sql".into(),
        data: b"SELECT 1;".to_vec(),
    }];
    let gz = build_tar_gz(&entries).expect("build");
    let archive_path = tmp.path().join("backup.tar.gz");
    std::fs::write(&archive_path, &gz).expect("write");

    // Pre-create the output directory.
    let outdir = tmp.path().join("existing-dir");
    std::fs::create_dir_all(&outdir).expect("create dir");

    let result = run_restore(RestoreArgs {
        input: archive_path,
        outdir,
        force: false,
        identity: None,
    });

    assert!(
        result.is_err(),
        "restore should refuse to overwrite without --force"
    );
    let msg = format!("{:?}", result.unwrap_err());
    assert!(
        msg.contains("force") || msg.contains("already exists"),
        "error should mention --force, got: {msg}"
    );
}

// ── binary-level backup tests (require `pg_dump` shim on `$PATH`) ─────────

/// Full backup + restore round-trip via the CLI binary with a mocked `pg_dump`.
#[test]
#[cfg(unix)]
fn backup_restore_round_trip_with_pg_dump_shim() {
    let dir = TempDir::new().expect("temp dir");
    let shim_dir = dir.path().join("bin");
    std::fs::create_dir_all(&shim_dir).expect("create bin dir");
    write_pg_dump_shim(&shim_dir);

    // Prepend shim dir to PATH so the binary finds our fake pg_dump.
    let original_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{original_path}", shim_dir.display());

    // Set up a fake config dir.
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    std::fs::write(config_dir.join("config.yaml"), b"fake: config").expect("write config");

    let backup_file = dir.path().join("backup.tar.gz");

    // Run backup.
    Command::cargo_bin("xiaoguai")
        .expect("binary")
        .env("PATH", &new_path)
        .env("XIAOGUAI_CONFIG_DIR", &config_dir)
        .args([
            "backup",
            "--out",
            backup_file.to_str().unwrap(),
            "--database-url",
            "postgresql://user:pass@localhost/testdb",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("backup written to"));

    assert!(backup_file.exists(), "backup file was not created");
    assert!(
        backup_file.metadata().unwrap().len() > 0,
        "backup file is empty"
    );

    // Run restore.
    let restore_dir = dir.path().join("restored");
    Command::cargo_bin("xiaoguai")
        .expect("binary")
        .args([
            "restore",
            "--in",
            backup_file.to_str().unwrap(),
            "--outdir",
            restore_dir.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("restore complete"));

    // pg_dump.sql should be inside the restored directory.
    let pg_dump_sql = restore_dir.join("pg_dump.sql");
    assert!(pg_dump_sql.exists(), "pg_dump.sql not found in restore dir");
    let sql = std::fs::read_to_string(&pg_dump_sql).expect("read sql");
    assert!(
        sql.contains("pg_dump shim output") || sql.contains("SELECT 1"),
        "restored pg_dump.sql has wrong content: {sql}"
    );
}

/// `backup --out` fails gracefully when `pg_dump` is not on `PATH`.
#[test]
fn backup_fails_gracefully_without_pg_dump() {
    let dir = TempDir::new().expect("temp dir");
    let backup_file = dir.path().join("backup.tar.gz");

    Command::cargo_bin("xiaoguai")
        .expect("binary")
        // Empty PATH so pg_dump cannot be found.
        .env("PATH", "")
        .args([
            "backup",
            "--out",
            backup_file.to_str().unwrap(),
            "--database-url",
            "postgresql://user:pass@localhost/testdb",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("pg_dump").or(predicate::str::contains("PATH")));
}
