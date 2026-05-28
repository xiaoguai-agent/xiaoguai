//! Focused unit tests for the high-risk crypto / integrity paths in
//! `commands::backup`: SHA-256 checksums, age encrypt/decrypt round-trips,
//! and restore error handling on corrupt / garbage / wrong-key input.
//!
//! These complement `backup_restore.rs` (which covers the happy-path archive
//! round-trip and overwrite refusal). All tests are hermetic: no network, no
//! real `pg_dump`, no real `~/.xiaoguai` directory — everything lives under a
//! `tempfile::TempDir`.

use age::secrecy::ExposeSecret;
use age::x25519::Identity;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use xiaoguai_cli::commands::backup::{
    age_decrypt, age_encrypt, build_tar_gz, config_dir, run_restore, ArchiveEntry, RestoreArgs,
};

// ── key-material helpers ────────────────────────────────────────────────────

/// Generate a fresh age X25519 keypair and write the identity + recipient
/// files into `dir`. Returns `(identity_path, recipient_path)`.
fn write_keypair(dir: &std::path::Path, name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let id = Identity::generate();
    let id_path = dir.join(format!("{name}.key"));
    let rec_path = dir.join(format!("{name}.pub"));
    std::fs::write(&id_path, id.to_string().expose_secret()).expect("write identity");
    std::fs::write(&rec_path, id.to_public().to_string()).expect("write recipient");
    (id_path, rec_path)
}

/// Build a *well-formed* backup archive: real entries plus a correctly
/// computed `sha256sum.txt`, mirroring what `run_backup` produces.
///
/// The checksum algorithm is reproduced here (it is private in the production
/// module) so we can assert that `run_restore` accepts an archive built to the
/// same contract.
fn build_valid_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive_entries: Vec<ArchiveEntry> = entries
        .iter()
        .map(|(p, d)| ArchiveEntry {
            path: (*p).to_string(),
            data: d.to_vec(),
        })
        .collect();

    // Replicate compute_manifest_checksum: sorted "path\0sha256(data)\n".
    let mut h = Sha256::new();
    let mut paths: Vec<&str> = archive_entries.iter().map(|e| e.path.as_str()).collect();
    paths.sort_unstable();
    for p in paths {
        if p == "sha256sum.txt" {
            continue;
        }
        if let Some(e) = archive_entries.iter().find(|e| e.path == p) {
            let mut inner = Sha256::new();
            inner.update(&e.data);
            let line = format!("{}\x00{}\n", p, hex::encode(inner.finalize()));
            h.update(line.as_bytes());
        }
    }
    let checksum = hex::encode(h.finalize());
    archive_entries.push(ArchiveEntry {
        path: "sha256sum.txt".into(),
        data: checksum.into_bytes(),
    });

    build_tar_gz(&archive_entries).expect("build tar.gz")
}

// ── SHA-256 known-answer ────────────────────────────────────────────────────

/// Pins the digest algorithm the production code uses for entry hashing.
///
/// `compute_manifest_checksum` is private, but it relies on `sha2::Sha256`
/// over the raw entry data. This is a known-answer test: NIST's canonical
/// `sha256("abc")` digest. If the hashing primitive ever changes, this fails.
#[test]
fn sha256_known_answer_abc() {
    let mut h = Sha256::new();
    h.update(b"abc");
    let digest = hex::encode(h.finalize());
    assert_eq!(
        digest, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        "SHA-256 of \"abc\" must match the NIST known answer"
    );
}

/// A correctly-built archive (valid `sha256sum.txt`) restores successfully and
/// extracts every entry to disk. This is the positive counterpart to the
/// tampered-archive rejection test.
#[test]
fn restore_accepts_matching_checksum_and_extracts() {
    let tmp = TempDir::new().expect("temp dir");
    let gz = build_valid_archive(&[
        ("pg_dump.sql", b"SELECT 1;"),
        ("config/config.yaml", b"database_url: postgres://x/y"),
    ]);
    let archive_path = tmp.path().join("good.tar.gz");
    std::fs::write(&archive_path, &gz).expect("write archive");

    let outdir = tmp.path().join("restore-out");
    run_restore(RestoreArgs {
        input: archive_path,
        outdir: outdir.clone(),
        force: false,
        identity: None,
    })
    .expect("restore should accept a valid checksum");

    let sql = std::fs::read(outdir.join("pg_dump.sql")).expect("read extracted sql");
    assert_eq!(sql, b"SELECT 1;");
    // Nested entry: restore must create parent dirs under outdir.
    let cfg = std::fs::read(outdir.join("config/config.yaml")).expect("read nested entry");
    assert_eq!(cfg, b"database_url: postgres://x/y");
}

/// A single-byte change to entry data (without updating the manifest) is
/// caught by restore's recompute-and-compare step.
#[test]
fn restore_rejects_corrupted_entry_data() {
    let tmp = TempDir::new().expect("temp dir");

    // Build a valid archive, then rebuild with the SAME checksum but mutated
    // data — i.e. the on-disk data no longer matches the stored manifest.
    let good_data = b"SELECT 1;";
    let mut inner = Sha256::new();
    inner.update(good_data);
    let mut h = Sha256::new();
    h.update(format!("pg_dump.sql\x00{}\n", hex::encode(inner.finalize())).as_bytes());
    let stored_checksum = hex::encode(h.finalize());

    let tampered = vec![
        ArchiveEntry {
            path: "pg_dump.sql".into(),
            data: b"SELECT 2;".to_vec(), // does NOT match stored checksum
        },
        ArchiveEntry {
            path: "sha256sum.txt".into(),
            data: stored_checksum.into_bytes(),
        },
    ];
    let gz = build_tar_gz(&tampered).expect("build");
    let archive_path = tmp.path().join("corrupt.tar.gz");
    std::fs::write(&archive_path, &gz).expect("write");

    let result = run_restore(RestoreArgs {
        input: archive_path,
        outdir: tmp.path().join("out"),
        force: false,
        identity: None,
    });
    let msg = format!("{:?}", result.expect_err("must reject corrupted data"));
    assert!(
        msg.contains("checksum"),
        "expected checksum error, got: {msg}"
    );
}

// ── age round-trip ──────────────────────────────────────────────────────────

/// Encrypt then decrypt returns the original bytes (binary-safe).
#[test]
fn age_round_trip_returns_original() {
    let tmp = TempDir::new().expect("temp dir");
    let (id_path, rec_path) = write_keypair(tmp.path(), "k");

    let plaintext: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
    let ct = age_encrypt(&plaintext, &rec_path).expect("encrypt");
    assert_ne!(ct, plaintext, "ciphertext must differ from plaintext");

    let pt = age_decrypt(&ct, &id_path).expect("decrypt");
    assert_eq!(pt, plaintext, "round-trip must preserve bytes exactly");
}

/// An age envelope encrypted to key A cannot be decrypted by key B's identity.
#[test]
fn age_decrypt_with_wrong_key_fails() {
    let tmp = TempDir::new().expect("temp dir");
    let (_id_a, rec_a) = write_keypair(tmp.path(), "a");
    let (id_b, _rec_b) = write_keypair(tmp.path(), "b");

    let ct = age_encrypt(b"top secret", &rec_a).expect("encrypt to A");
    let result = age_decrypt(&ct, &id_b);
    assert!(
        result.is_err(),
        "decryption with the wrong identity must fail"
    );
}

/// A recipient file with no usable keys is rejected with a clear message
/// (comments and blank lines are skipped, leaving nothing valid).
#[test]
fn age_encrypt_rejects_recipient_file_without_keys() {
    let tmp = TempDir::new().expect("temp dir");
    let rec_path = tmp.path().join("empty.pub");
    std::fs::write(&rec_path, "# just a comment\n\n   \n").expect("write");

    let result = age_encrypt(b"data", &rec_path);
    let msg = format!(
        "{:?}",
        result.expect_err("must reject keyless recipient file")
    );
    assert!(
        msg.contains("no valid age recipients"),
        "expected 'no valid age recipients', got: {msg}"
    );
}

/// A garbage recipient key produces a parse error mentioning the key, not a
/// panic.
#[test]
fn age_encrypt_rejects_garbage_recipient_key() {
    let tmp = TempDir::new().expect("temp dir");
    let rec_path = tmp.path().join("bad.pub");
    std::fs::write(&rec_path, "not-an-age-key").expect("write");

    let result = age_encrypt(b"data", &rec_path);
    let msg = format!("{:?}", result.expect_err("must reject garbage recipient"));
    assert!(
        msg.contains("parse recipient") || msg.contains("X25519"),
        "expected parse-recipient error, got: {msg}"
    );
}

/// Decrypting non-age bytes fails with an error rather than panicking.
#[test]
fn age_decrypt_rejects_non_age_input() {
    let tmp = TempDir::new().expect("temp dir");
    let (id_path, _rec) = write_keypair(tmp.path(), "k");

    let result = age_decrypt(b"this is not an age file at all", &id_path);
    assert!(result.is_err(), "non-age input must error, not panic");
}

/// An identity file containing no valid X25519 keys is rejected clearly.
#[test]
fn age_decrypt_rejects_identity_file_without_keys() {
    let tmp = TempDir::new().expect("temp dir");
    let id_path = tmp.path().join("empty.key");
    std::fs::write(&id_path, "# comment only\n").expect("write");

    // Any ciphertext bytes — the identity parse happens before decryption.
    let result = age_decrypt(b"\x00\x01\x02", &id_path);
    let msg = format!("{:?}", result.expect_err("must reject keyless identity"));
    assert!(
        msg.contains("no valid age X25519 identities") || msg.contains("identity"),
        "expected identity error, got: {msg}"
    );
}

// ── encrypted backup → restore (end-to-end through library fns) ──────────────

/// A valid archive encrypted with age can be decrypted and restored when the
/// matching identity is supplied to `run_restore`.
#[test]
fn encrypted_archive_round_trips_through_restore() {
    let tmp = TempDir::new().expect("temp dir");
    let (id_path, rec_path) = write_keypair(tmp.path(), "k");

    let gz = build_valid_archive(&[("pg_dump.sql", b"SELECT 42;")]);
    let ct = age_encrypt(&gz, &rec_path).expect("encrypt archive");
    let enc_path = tmp.path().join("backup.tar.gz.age");
    std::fs::write(&enc_path, &ct).expect("write encrypted archive");

    let outdir = tmp.path().join("restore-out");
    run_restore(RestoreArgs {
        input: enc_path,
        outdir: outdir.clone(),
        force: false,
        identity: Some(id_path),
    })
    .expect("encrypted restore should succeed with the right identity");

    let sql = std::fs::read(outdir.join("pg_dump.sql")).expect("read extracted sql");
    assert_eq!(sql, b"SELECT 42;");
}

/// Supplying the wrong identity to `run_restore` for an encrypted archive
/// surfaces the decryption failure (no panic, no silent empty restore).
#[test]
fn encrypted_restore_with_wrong_identity_fails() {
    let tmp = TempDir::new().expect("temp dir");
    let (_id_a, rec_a) = write_keypair(tmp.path(), "a");
    let (id_b, _rec_b) = write_keypair(tmp.path(), "b");

    let gz = build_valid_archive(&[("pg_dump.sql", b"SELECT 1;")]);
    let ct = age_encrypt(&gz, &rec_a).expect("encrypt to A");
    let enc_path = tmp.path().join("backup.tar.gz.age");
    std::fs::write(&enc_path, &ct).expect("write");

    let result = run_restore(RestoreArgs {
        input: enc_path,
        outdir: tmp.path().join("out"),
        force: false,
        identity: Some(id_b),
    });
    let msg = format!("{:?}", result.expect_err("wrong identity must fail"));
    assert!(
        msg.contains("decryption") || msg.contains("decrypt"),
        "expected decryption error, got: {msg}"
    );
}

// ── restore garbage / missing input ──────────────────────────────────────────

/// Restoring from a non-existent input file errors with a read failure, not a
/// panic.
#[test]
fn restore_missing_input_errors() {
    let tmp = TempDir::new().expect("temp dir");
    let result = run_restore(RestoreArgs {
        input: tmp.path().join("does-not-exist.tar.gz"),
        outdir: tmp.path().join("out"),
        force: false,
        identity: None,
    });
    let msg = format!("{:?}", result.expect_err("missing input must error"));
    assert!(
        msg.contains("read backup file"),
        "expected read-backup-file error, got: {msg}"
    );
}

/// Restoring from bytes that are not valid gzip fails at decompression with a
/// clear error rather than panicking.
#[test]
fn restore_garbage_gzip_errors() {
    let tmp = TempDir::new().expect("temp dir");
    let path = tmp.path().join("garbage.tar.gz");
    std::fs::write(&path, b"definitely not gzip data \xff\x00\xff").expect("write");

    let result = run_restore(RestoreArgs {
        input: path,
        outdir: tmp.path().join("out"),
        force: false,
        identity: None,
    });
    let msg = format!("{:?}", result.expect_err("garbage gzip must error"));
    assert!(
        msg.contains("gzip") || msg.contains("decompress"),
        "expected gzip/decompress error, got: {msg}"
    );
}

// ── config_dir precedence ────────────────────────────────────────────────────

/// `XIAOGUAI_CONFIG_DIR` takes precedence over the `$HOME/.xiaoguai` default.
///
/// Note: env vars are process-global, so this test sets and restores the var.
/// It does not run concurrently-safe assertions against other env-reading tests
/// in this file (none read this var).
#[test]
fn config_dir_honours_env_override() {
    let tmp = TempDir::new().expect("temp dir");
    let custom = tmp.path().join("custom-config");

    let prev = std::env::var_os("XIAOGUAI_CONFIG_DIR");
    std::env::set_var("XIAOGUAI_CONFIG_DIR", &custom);
    let resolved = config_dir();
    // Restore before asserting so a failed assert can't leak the override.
    match prev {
        Some(v) => std::env::set_var("XIAOGUAI_CONFIG_DIR", v),
        None => std::env::remove_var("XIAOGUAI_CONFIG_DIR"),
    }

    assert_eq!(resolved, custom, "config_dir must honour the env override");
}
