//! `xiaoguai backup` / `xiaoguai restore` — data protection subcommands.
//!
//! # Backup
//!
//! Produces a `<file>.tar.gz` containing:
//!
//! - `pg_dump.sql` — plain-text `pg_dump` of the target database
//! - `config/` — everything under `~/.xiaoguai/` (or `$XIAOGUAI_CONFIG_DIR`)
//! - `audit.db` — audit `SQLite` snapshot (if it exists)
//!
//! Optional `--encrypt <recipient.age>` wraps the tarball in an age envelope.
//! The output file is written atomically (write temp → rename).
//!
//! # Restore
//!
//! Unpacks a previously created backup.  Validates the SHA-256 checksum stored
//! in `sha256sum.txt` inside the archive before extracting anything.
//! Refuses to overwrite an existing destination directory unless `--force` is
//! given.  Both subcommands append a log entry to `~/.xiaoguai/audit.log`.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

// ── helpers ────────────────────────────────────────────────────────────────

/// Resolve the xiaoguai config directory.
///
/// Priority: `$XIAOGUAI_CONFIG_DIR` → `~/.xiaoguai`.
#[must_use]
pub fn config_dir() -> PathBuf {
    if let Ok(d) = std::env::var("XIAOGUAI_CONFIG_DIR") {
        return PathBuf::from(d);
    }
    if let Some(home) = dirs_next_home() {
        return home.join(".xiaoguai");
    }
    PathBuf::from(".xiaoguai")
}

fn dirs_next_home() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

/// Append one line to the audit log (`~/.xiaoguai/audit.log`).  Failures are
/// non-fatal — we warn to stderr and continue.
pub fn audit_log(msg: &str) {
    let log_path = config_dir().join("audit.log");
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let ts = chrono::Utc::now().to_rfc3339();
    let line = format!("{ts} {msg}\n");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        let _ = f.write_all(line.as_bytes());
    } else {
        eprintln!("warn: could not append to audit log {}", log_path.display());
    }
}

// ── pg_dump ────────────────────────────────────────────────────────────────

/// Find `pg_dump` on `$PATH`.
fn find_pg_dump() -> Result<PathBuf> {
    which_in_path("pg_dump").context(
        "pg_dump not found on PATH. Install postgresql-client and ensure pg_dump is in PATH.",
    )
}

fn which_in_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join(name);
            if candidate.is_file() {
                Some(candidate)
            } else {
                None
            }
        })
    })
}

/// Run `pg_dump` and return the SQL as a byte vector.
///
/// `database_url` is passed via `PGPASSWORD` / `PGHOST` / … env vars (or a
/// full connection URI).  We spawn `pg_dump` with `--format=plain` so the dump
/// is a human-readable SQL file.
///
/// # Errors
/// Returns an error if `pg_dump` cannot be spawned, exits non-zero, or the
/// output is not valid UTF-8.
pub fn run_pg_dump(pg_dump_path: &Path, database_url: &str) -> Result<Vec<u8>> {
    // Parse the URL to extract components pg_dump understands.
    // Simplest approach: pass the full URL via the `--dbname` flag, which
    // pg_dump 9.3+ accepts as a connection string URI.
    let output = std::process::Command::new(pg_dump_path)
        .args(["--format=plain", "--no-password"])
        .arg("--dbname")
        .arg(database_url)
        .output()
        .context("spawn pg_dump")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("pg_dump exited with {}: {stderr}", output.status);
    }
    Ok(output.stdout)
}

// ── SHA-256 ────────────────────────────────────────────────────────────────

fn sha256_of(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

// ── tar.gz builder ─────────────────────────────────────────────────────────

/// In-memory representation of one file to include in the archive.
pub struct ArchiveEntry {
    pub path: String,
    pub data: Vec<u8>,
}

/// Build an uncompressed tar in memory and then gzip it.
///
/// # Errors
/// Returns an error if tar header construction or gzip compression fails.
pub fn build_tar_gz(entries: &[ArchiveEntry]) -> Result<Vec<u8>> {
    // Build tar in a Vec<u8>.
    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        for entry in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(entry.data.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            );
            header.set_cksum();
            builder
                .append_data(&mut header, &entry.path, entry.data.as_slice())
                .with_context(|| format!("append archive entry {}", entry.path))?;
        }
        builder.finish().context("finalise tar")?;
    }

    // Gzip the tar bytes.
    let mut gz_bytes = Vec::new();
    {
        let mut encoder =
            flate2::write::GzEncoder::new(&mut gz_bytes, flate2::Compression::default());
        encoder.write_all(&tar_bytes).context("gzip compress tar")?;
        encoder.finish().context("finalise gzip")?;
    }
    Ok(gz_bytes)
}

// ── age encryption ─────────────────────────────────────────────────────────

/// Encrypt `plaintext` to the age recipient at `recipient_path`.
///
/// `recipient_path` must be a file containing one or more age public keys
/// (one per line, X25519 or SSH).
///
/// # Errors
/// Returns an error if the recipient file cannot be read, parsed, or the
/// encryption step fails.
///
/// # Panics
/// Panics if the in-memory age encryption writer panics internally.
pub fn age_encrypt(plaintext: &[u8], recipient_path: &Path) -> Result<Vec<u8>> {
    let recipient_str = std::fs::read_to_string(recipient_path)
        .with_context(|| format!("read recipient file {}", recipient_path.display()))?;

    let mut recipients: Vec<Box<dyn age::Recipient + Send>> = Vec::new();
    for line in recipient_str.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // We support X25519 age recipients (age1… public keys).
        // SSH recipient support requires the "ssh" feature of the age crate
        // and is not enabled in this build.
        let r: Box<dyn age::Recipient + Send> = trimmed
            .parse::<age::x25519::Recipient>()
            .map(|r| -> Box<dyn age::Recipient + Send> { Box::new(r) })
            .map_err(|e| anyhow::anyhow!("{e}"))
            .with_context(|| format!("parse recipient key: {trimmed}. Only age X25519 public keys (age1…) are supported."))?;
        recipients.push(r);
    }

    if recipients.is_empty() {
        bail!(
            "no valid age recipients found in {}",
            recipient_path.display()
        );
    }

    let mut ciphertext = Vec::new();
    let encryptor =
        age::Encryptor::with_recipients(recipients).expect("at least one recipient present");
    {
        let mut writer = encryptor
            .wrap_output(&mut ciphertext)
            .context("initialise age encryption")?;
        writer.write_all(plaintext).context("encrypt data")?;
        writer.finish().context("finalise age encryption")?;
    }
    Ok(ciphertext)
}

// ── age decryption ─────────────────────────────────────────────────────────

/// Decrypt an age-encrypted blob using the identity file at `identity_path`.
///
/// The identity file must contain age X25519 secret keys (AGE-SECRET-KEY-… lines).
///
/// # Errors
/// Returns an error if the identity file cannot be read, parsed, or the
/// decryption step fails.
pub fn age_decrypt(ciphertext: &[u8], identity_path: &Path) -> Result<Vec<u8>> {
    let id_str = std::fs::read_to_string(identity_path)
        .with_context(|| format!("read identity file {}", identity_path.display()))?;

    // Parse X25519 identities directly (we don't depend on the `ssh` feature).
    let identities: Vec<age::x25519::Identity> = id_str
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))
        .map(|l| {
            l.trim()
                .parse::<age::x25519::Identity>()
                .map_err(|e| anyhow::anyhow!("parse X25519 identity: {e}"))
        })
        .collect::<Result<Vec<_>>>()?;

    if identities.is_empty() {
        bail!(
            "no valid age X25519 identities found in {}",
            identity_path.display()
        );
    }

    let age::Decryptor::Recipients(decryptor) =
        age::Decryptor::new(ciphertext).context("create age decryptor")?
    else {
        bail!("passphrase-encrypted age files are not supported; use a key-based recipient");
    };

    let mut plaintext = Vec::new();
    let mut reader = decryptor
        .decrypt(identities.iter().map(|i| i as &dyn age::Identity))
        .context("age decryption failed")?;
    reader
        .read_to_end(&mut plaintext)
        .context("read decrypted data")?;
    Ok(plaintext)
}

// ── backup ─────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct BackupArgs {
    /// Output path for the `.tar.gz` (or `.tar.gz.age` if encrypted).
    pub out: PathBuf,
    /// Database connection URL (postgresql://…).
    pub database_url: String,
    /// Optional age public-key file for encryption.
    pub encrypt: Option<PathBuf>,
}

/// Run `xiaoguai backup`.
///
/// Returns the final output path written.
///
/// # Errors
/// Returns an error if any step (`pg_dump`, tar, age encrypt, file write) fails.
#[allow(
    clippy::needless_pass_by_value,
    reason = "public API — callers construct and pass by value"
)]
pub fn run_backup(args: BackupArgs) -> Result<PathBuf> {
    let pg_dump_bin = find_pg_dump()?;

    // 1. pg_dump
    let sql = run_pg_dump(&pg_dump_bin, &args.database_url)
        .context("pg_dump failed — check DATABASE_URL and pg_dump PATH")?;

    // 2. Collect config directory.
    let cfg_dir = config_dir();
    let mut entries: Vec<ArchiveEntry> = Vec::new();

    // pg_dump goes first.
    entries.push(ArchiveEntry {
        path: "pg_dump.sql".into(),
        data: sql,
    });

    // Config dir contents.
    if cfg_dir.is_dir() {
        collect_dir(&cfg_dir, "config", &mut entries)?;
    }

    // Audit DB snapshot (optional).
    let audit_db = cfg_dir.join("audit.db");
    if audit_db.is_file() {
        let data = std::fs::read(&audit_db)
            .with_context(|| format!("read audit DB {}", audit_db.display()))?;
        entries.push(ArchiveEntry {
            path: "audit.db".into(),
            data,
        });
    }

    // 3. Checksum manifest (over all entry paths + data).
    let checksum = compute_manifest_checksum(&entries);
    entries.push(ArchiveEntry {
        path: "sha256sum.txt".into(),
        data: checksum.into_bytes(),
    });

    // 4. Build tar.gz.
    let mut archive_bytes = build_tar_gz(&entries).context("build tar.gz")?;

    // 5. Optional age encryption.
    let final_path = if let Some(rec_path) = &args.encrypt {
        archive_bytes = age_encrypt(&archive_bytes, rec_path).context("age encryption failed")?;
        let mut p = args.out.clone();
        let stem = p
            .file_name()
            .map_or_else(|| "backup".into(), |s| s.to_string_lossy().into_owned());
        p.set_file_name(format!("{stem}.age"));
        p
    } else {
        args.out.clone()
    };

    // 6. Atomic write (temp → rename).
    let parent = final_path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = tempfile::NamedTempFile::new_in(parent).context("create temp file")?;
    tmp.as_file().metadata().ok(); // pre-flight
    std::fs::write(tmp.path(), &archive_bytes)
        .with_context(|| format!("write backup to temp file {}", tmp.path().display()))?;
    tmp.persist(&final_path)
        .with_context(|| format!("rename temp file to {}", final_path.display()))?;

    audit_log(&format!(
        "backup created path={} size={}",
        final_path.display(),
        archive_bytes.len()
    ));
    Ok(final_path)
}

/// Recursively collect files under `dir` as archive entries with paths
/// relative to `prefix`.
fn collect_dir(dir: &Path, prefix: &str, entries: &mut Vec<ArchiveEntry>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("read dir {}", dir.display()))? {
        let entry = entry.with_context(|| format!("read dir entry in {}", dir.display()))?;
        let path = entry.path();
        let file_name = entry.file_name();
        let child_prefix = format!("{prefix}/{}", file_name.to_string_lossy());
        if path.is_dir() {
            collect_dir(&path, &child_prefix, entries)?;
        } else {
            let data =
                std::fs::read(&path).with_context(|| format!("read file {}", path.display()))?;
            entries.push(ArchiveEntry {
                path: child_prefix,
                data,
            });
        }
    }
    Ok(())
}

/// SHA-256 over `path\x00sha256(data)\n` for each entry (sorted by path).
fn compute_manifest_checksum(entries: &[ArchiveEntry]) -> String {
    let mut h = Sha256::new();
    let mut paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
    paths.sort_unstable();
    for p in paths {
        if p == "sha256sum.txt" {
            continue; // don't include the checksum file itself
        }
        if let Some(e) = entries.iter().find(|e| e.path == p) {
            let line = format!("{}\x00{}\n", p, sha256_of(&e.data));
            h.update(line.as_bytes());
        }
    }
    hex::encode(h.finalize())
}

// ── restore ────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct RestoreArgs {
    /// Input `.tar.gz` (or `.tar.gz.age`) path.
    pub input: PathBuf,
    /// Directory to extract into.  Must not exist unless `--force`.
    pub outdir: PathBuf,
    /// If true, overwrite existing outdir.
    pub force: bool,
    /// Optional age identity file for decryption.
    pub identity: Option<PathBuf>,
}

/// Run `xiaoguai restore`.
///
/// # Errors
/// Returns an error if extraction, decryption, or file writes fail.
#[allow(
    clippy::needless_pass_by_value,
    reason = "public API — callers construct and pass by value"
)]
pub fn run_restore(args: RestoreArgs) -> Result<()> {
    if args.outdir.exists() && !args.force {
        bail!(
            "output directory {} already exists. Pass --force to overwrite.",
            args.outdir.display()
        );
    }

    // 1. Read archive bytes.
    let mut archive_bytes = std::fs::read(&args.input)
        .with_context(|| format!("read backup file {}", args.input.display()))?;

    // 2. Optional age decryption.
    if let Some(id_path) = &args.identity {
        archive_bytes = age_decrypt(&archive_bytes, id_path).context("age decryption failed")?;
    }

    // 3. Decompress gzip → tar bytes.
    let mut tar_bytes = Vec::new();
    {
        let mut decoder = flate2::read::GzDecoder::new(archive_bytes.as_slice());
        decoder
            .read_to_end(&mut tar_bytes)
            .context("gzip decompress failed")?;
    }

    // 4. Parse tar; collect all entries into memory so we can checksum first.
    let mut archive = tar::Archive::new(tar_bytes.as_slice());
    let mut file_map: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();
    for entry in archive.entries().context("read tar entries")? {
        let mut entry = entry.context("read tar entry")?;
        let path_str = entry
            .path()
            .context("tar entry path")?
            .to_string_lossy()
            .into_owned();
        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .context("read tar entry data")?;
        file_map.insert(path_str, data);
    }

    // 5. Validate checksum.
    let stored_hex = file_map
        .get("sha256sum.txt")
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default();
    let stored_hex = stored_hex.trim().to_string();

    // Recompute from in-memory map (same algorithm as backup).
    let computed_hex = {
        let mut h = Sha256::new();
        let mut paths: Vec<&str> = file_map.keys().map(String::as_str).collect();
        paths.sort_unstable();
        for p in &paths {
            if *p == "sha256sum.txt" {
                continue;
            }
            if let Some(data) = file_map.get(*p) {
                let line = format!("{}\x00{}\n", p, sha256_of(data));
                h.update(line.as_bytes());
            }
        }
        hex::encode(h.finalize())
    };

    if stored_hex != computed_hex {
        bail!(
            "checksum mismatch: stored={stored_hex} computed={computed_hex}. \
             The archive may be corrupted or tampered with."
        );
    }

    // 6. Extract.
    if args.force && args.outdir.exists() {
        std::fs::remove_dir_all(&args.outdir)
            .with_context(|| format!("remove existing dir {}", args.outdir.display()))?;
    }
    std::fs::create_dir_all(&args.outdir)
        .with_context(|| format!("create output dir {}", args.outdir.display()))?;

    for (path_str, data) in &file_map {
        let dest = args.outdir.join(path_str);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create parent dirs for {}", dest.display()))?;
        }
        std::fs::write(&dest, data)
            .with_context(|| format!("write extracted file {}", dest.display()))?;
    }

    audit_log(&format!(
        "restore completed input={} outdir={}",
        args.input.display(),
        args.outdir.display()
    ));
    Ok(())
}
