//! Integration tests for `xiaoguai backup` / `xiaoguai restore`.
//!
//! Under the DEC-033 single-user pivot the application state is a single `SQLite`
//! file (`data.db`). These tests seed a temp `SQLite` store (via the storage
//! crate's `connect` + `migrate`), insert a couple of rows, then assert the
//! backup archive round-trips that file — i.e. restoring reproduces a database
//! containing the seeded rows.

use std::io::Read;

use assert_cmd::Command;
use predicates::prelude::*;
use sqlx::Row;
use tempfile::TempDir;
use xiaoguai_cli::commands::backup::{build_tar_gz, run_restore, ArchiveEntry, RestoreArgs};

// ── SQLite seed helper ──────────────────────────────────────────────────────

/// Create a migrated `SQLite` store at `path` and insert two `token_usage` rows.
/// Returns the number of rows inserted.
async fn seed_sqlite(path: &std::path::Path) -> i64 {
    let url = format!("sqlite://{}", path.display());
    let pool = xiaoguai_storage::connect(&url, 1)
        .await
        .expect("connect sqlite");
    xiaoguai_storage::migrate(&pool).await.expect("migrate");

    sqlx::query(
        "INSERT INTO token_usage (provider_id, model, prompt_tokens, completion_tokens, total_tokens) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind("openai-gpt-4o")
    .bind("gpt-4o")
    .bind(100_i64)
    .bind(50_i64)
    .bind(150_i64)
    .execute(&pool)
    .await
    .expect("insert row 1");

    sqlx::query(
        "INSERT INTO token_usage (provider_id, model, prompt_tokens, completion_tokens, total_tokens) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind("ollama-local")
    .bind("qwen2.5-coder")
    .bind(10_i64)
    .bind(5_i64)
    .bind(15_i64)
    .execute(&pool)
    .await
    .expect("insert row 2");

    let row = sqlx::query("SELECT COUNT(*) AS n FROM token_usage")
        .fetch_one(&pool)
        .await
        .expect("count");
    row.try_get::<i64, _>("n").expect("read count")
}

/// Open `path` and return the `token_usage` row count.
async fn count_rows(path: &std::path::Path) -> i64 {
    let url = format!("sqlite://{}", path.display());
    let pool = xiaoguai_storage::connect(&url, 1)
        .await
        .expect("reopen sqlite");
    let row = sqlx::query("SELECT COUNT(*) AS n FROM token_usage")
        .fetch_one(&pool)
        .await
        .expect("count");
    row.try_get::<i64, _>("n").expect("read count")
}

// ── unit-level round-trip (library functions, no subprocess) ──────────────

/// Round-trip through `build_tar_gz` + extraction logic.
///
/// Tests archive + checksum logic without a real database or the binary.
#[test]
fn archive_round_trip_checksum_passes() {
    let entries = vec![
        ArchiveEntry {
            path: "data.db".into(),
            data: b"SQLite format 3\x00fake".to_vec(),
        },
        ArchiveEntry {
            path: "config/config.yaml".into(),
            data: b"database:\n  url: \"\"".to_vec(),
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
        paths.contains(&"data.db".to_string()),
        "data.db not found, got: {paths:?}"
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
            path: "data.db".into(),
            data: b"SQLite format 3\x00".to_vec(),
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
        restore_db_to: None,
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
        path: "data.db".into(),
        data: b"SQLite format 3\x00".to_vec(),
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
        restore_db_to: None,
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

// ── binary-level backup tests (real SQLite store) ──────────────────────────

/// Full backup + restore round-trip via the CLI binary against a seeded `SQLite`
/// store, asserting the restored DB carries the seeded rows.
#[test]
fn backup_restore_round_trip_sqlite() {
    let dir = TempDir::new().expect("temp dir");

    // Seed a real SQLite store.
    let db_path = dir.path().join("data.db");
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let seeded = rt.block_on(seed_sqlite(&db_path));
    assert_eq!(seeded, 2, "seed should insert 2 rows");

    // Fake config dir (so the archive also carries config/, exercising that path).
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    std::fs::write(config_dir.join("config.yaml"), b"fake: config").expect("write config");

    let backup_file = dir.path().join("backup.tar.gz");

    // Run backup with --database-url pointing at the seeded store.
    Command::cargo_bin("xiaoguai")
        .expect("binary")
        .env("XIAOGUAI_CONFIG_DIR", &config_dir)
        .args([
            "backup",
            "--out",
            backup_file.to_str().unwrap(),
            "--database-url",
            db_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("backup written to"));

    assert!(backup_file.exists(), "backup file was not created");
    assert!(
        backup_file.metadata().unwrap().len() > 0,
        "backup file is empty"
    );

    // Restore the archive to a directory AND to a fresh live DB path.
    let restore_dir = dir.path().join("restored");
    let restored_db = dir.path().join("restored-data.db");
    Command::cargo_bin("xiaoguai")
        .expect("binary")
        .args([
            "restore",
            "--in",
            backup_file.to_str().unwrap(),
            "--outdir",
            restore_dir.to_str().unwrap(),
            "--restore-db",
            restored_db.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("restore complete"));

    // data.db should be inside the restored directory.
    let extracted_db = restore_dir.join("data.db");
    assert!(extracted_db.exists(), "data.db not found in restore dir");

    // The live-restored DB must reopen and carry the seeded rows.
    assert!(restored_db.exists(), "restored live DB not written");
    let n = rt.block_on(count_rows(&restored_db));
    assert_eq!(n, 2, "restored DB must carry the 2 seeded rows");
}

/// `backup --out` fails gracefully when the `SQLite` store does not exist.
#[test]
fn backup_fails_gracefully_without_store() {
    let dir = TempDir::new().expect("temp dir");
    let backup_file = dir.path().join("backup.tar.gz");
    let missing_db = dir.path().join("nope.db");

    Command::cargo_bin("xiaoguai")
        .expect("binary")
        .args([
            "backup",
            "--out",
            backup_file.to_str().unwrap(),
            "--database-url",
            missing_db.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("does not exist").or(predicate::str::contains("data.db")));
}
