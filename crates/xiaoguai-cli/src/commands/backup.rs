//! `xiaoguai backup` / `xiaoguai restore` — data protection subcommands.
//!
//! Under the DEC-033 single-user pivot the entire application state is one
//! `SQLite` file (`~/.xiaoguai/data.db` by default).
//!
//! # Backup
//!
//! Produces a `<file>.tar.gz` containing:
//!
//! - `data.db` — a consistent `SQLite` snapshot taken with `VACUUM INTO`, which
//!   is WAL-safe and yields a single clean file with no `-wal`/`-shm` sidecars
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
use std::str::FromStr;

use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use sqlx::{ConnectOptions, Connection};

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

// ── SQLite snapshot ──────────────────────────────────────────────────────────

/// Turn a configured `database.url` into the concrete `SQLite` file path.
///
/// Mirrors `xiaoguai_storage::db`'s private resolver: an empty string or the
/// literal `"default"` resolves to [`xiaoguai_storage::db::default_db_path`];
/// otherwise a `sqlite://` / `sqlite:` prefix is stripped and the remainder is
/// used as a filesystem path.
#[must_use]
pub fn resolve_sqlite_path(url: &str) -> PathBuf {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed == "default" {
        return xiaoguai_storage::db::default_db_path();
    }
    let stripped = trimmed
        .strip_prefix("sqlite://")
        .or_else(|| trimmed.strip_prefix("sqlite:"))
        .unwrap_or(trimmed);
    if stripped.is_empty() || stripped == ":memory:" {
        return xiaoguai_storage::db::default_db_path();
    }
    PathBuf::from(stripped)
}

/// Take a consistent `SQLite` snapshot of `db_path` into a fresh file and return
/// its bytes.
///
/// Uses `VACUUM INTO`, which is safe with respect to WAL mode: it produces a
/// single, fully-checkpointed database file with no `-wal`/`-shm` sidecars and
/// without interrupting concurrent readers. The snapshot is written to a temp
/// file (in the same parent dir as the source so it stays on one filesystem),
/// read back into memory, and the temp file is removed.
///
/// # Errors
/// Returns an error if the source DB is missing, cannot be opened, the
/// `VACUUM INTO` fails, or the snapshot file cannot be read.
pub fn snapshot_sqlite(db_path: &Path) -> Result<Vec<u8>> {
    if !db_path.is_file() {
        bail!(
            "SQLite store {} does not exist — nothing to back up. \
             Run the app once (or `xiaoguai serve`) to create it.",
            db_path.display()
        );
    }

    // Snapshot target: a sibling temp file (same dir → same filesystem so the
    // SQLite backup is atomic-on-rename and we don't cross device boundaries).
    let parent = db_path.parent().unwrap_or_else(|| Path::new("."));
    let snap = tempfile::Builder::new()
        .prefix(".xiaoguai-backup-")
        .suffix(".db")
        .tempfile_in(parent)
        .context("create snapshot temp file")?;
    // VACUUM INTO requires the destination not to exist yet.
    let snap_path = snap.path().to_path_buf();
    drop(snap); // remove the placeholder; keep the path
    let _ = std::fs::remove_file(&snap_path);

    run_vacuum_into(db_path, &snap_path).context("VACUUM INTO snapshot failed")?;

    let bytes = std::fs::read(&snap_path)
        .with_context(|| format!("read snapshot file {}", snap_path.display()))?;
    let _ = std::fs::remove_file(&snap_path);
    Ok(bytes)
}

/// Open `src` read-only and run `VACUUM INTO 'dest'`.
///
/// `run_backup` is a synchronous public API that is also invoked from inside
/// the binary's async `main`. Driving a runtime here directly would panic with
/// "Cannot start a runtime from within a runtime", so the snapshot work runs on
/// a dedicated OS thread that owns its own current-thread runtime.
fn run_vacuum_into(src: &Path, dest: &Path) -> Result<()> {
    let src = src.to_path_buf();
    let dest = dest.to_path_buf();
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .context("build snapshot runtime")?;
                rt.block_on(async move {
                    let opts =
                        SqliteConnectOptions::from_str(&format!("sqlite://{}", src.display()))
                            .unwrap_or_else(|_| SqliteConnectOptions::new().filename(&src))
                            .filename(&src)
                            .read_only(true);
                    let mut conn: SqliteConnection = opts
                        .connect()
                        .await
                        .with_context(|| format!("open SQLite store {}", src.display()))?;
                    // VACUUM INTO does not accept bind parameters; the
                    // destination is a tempfile path we created, not user input.
                    let dest_lit = dest.display().to_string().replace('\'', "''");
                    sqlx::query(&format!("VACUUM INTO '{dest_lit}'"))
                        .execute(&mut conn)
                        .await
                        .context("execute VACUUM INTO")?;
                    conn.close().await.context("close snapshot connection")?;
                    Ok::<(), anyhow::Error>(())
                })
            })
            .join()
            .map_err(|_| anyhow::anyhow!("snapshot thread panicked"))?
    })
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
    /// Configured `database.url`. Empty / `"default"` → the default store path.
    pub database_url: String,
    /// Optional age public-key file for encryption.
    pub encrypt: Option<PathBuf>,
}

/// Run `xiaoguai backup`.
///
/// Returns the final output path written.
///
/// # Errors
/// Returns an error if any step (`SQLite` snapshot, tar, age encrypt, file write)
/// fails.
#[allow(
    clippy::needless_pass_by_value,
    reason = "public API — callers construct and pass by value"
)]
pub fn run_backup(args: BackupArgs) -> Result<PathBuf> {
    // 1. SQLite snapshot (WAL-safe via VACUUM INTO).
    let db_path = resolve_sqlite_path(&args.database_url);
    let db_bytes = snapshot_sqlite(&db_path)?;

    // 2. Collect config directory.
    let cfg_dir = config_dir();
    let mut entries: Vec<ArchiveEntry> = Vec::new();

    // The SQLite store snapshot goes first.
    entries.push(ArchiveEntry {
        path: "data.db".into(),
        data: db_bytes,
    });

    // Config dir contents. Skip the live SQLite store + its WAL/SHM sidecars:
    // the clean `data.db` snapshot above is the canonical copy, and the live
    // file may carry an uncheckpointed WAL we deliberately avoided.
    if cfg_dir.is_dir() {
        let skip = sqlite_sidecar_names(&db_path, &cfg_dir);
        collect_dir(&cfg_dir, "config", &skip, &mut entries)?;
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

/// File names (relative to the config dir) to skip when collecting it: the live
/// `SQLite` store plus its `-wal`/`-shm` sidecars, when they live under `cfg_dir`.
fn sqlite_sidecar_names(db_path: &Path, cfg_dir: &Path) -> Vec<String> {
    let mut skip = Vec::new();
    if db_path.parent() == Some(cfg_dir) {
        if let Some(name) = db_path.file_name().and_then(|n| n.to_str()) {
            skip.push(name.to_string());
            skip.push(format!("{name}-wal"));
            skip.push(format!("{name}-shm"));
        }
    }
    skip
}

/// Recursively collect files under `dir` as archive entries with paths
/// relative to `prefix`, skipping any top-level file whose name is in `skip`.
fn collect_dir(
    dir: &Path,
    prefix: &str,
    skip: &[String],
    entries: &mut Vec<ArchiveEntry>,
) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("read dir {}", dir.display()))? {
        let entry = entry.with_context(|| format!("read dir entry in {}", dir.display()))?;
        let path = entry.path();
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();
        if path.is_file() && skip.iter().any(|s| s.as_str() == name_str) {
            continue;
        }
        let child_prefix = format!("{prefix}/{name_str}");
        if path.is_dir() {
            // Sidecar skipping only applies at the config-dir top level.
            collect_dir(&path, &child_prefix, &[], entries)?;
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
    /// Directory to extract the full archive into.  Must not exist unless
    /// `--force`.
    pub outdir: PathBuf,
    /// If true, overwrite an existing outdir and an existing live `SQLite` store.
    pub force: bool,
    /// Optional age identity file for decryption.
    pub identity: Option<PathBuf>,
    /// When set, the archived `data.db` payload is also written to this live
    /// `SQLite` path (the running store). An existing file is preserved as
    /// `<path>.bak` first, and is only overwritten when `force` is true.
    pub restore_db_to: Option<PathBuf>,
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

    // 7. Optionally restore the SQLite store to its live path.
    if let Some(db_target) = &args.restore_db_to {
        let db_bytes = file_map.get("data.db").ok_or_else(|| {
            anyhow::anyhow!(
                "archive has no data.db entry — cannot restore the SQLite store. \
                 (Archives created before the SQLite pivot carry pg_dump.sql instead.)"
            )
        })?;
        restore_sqlite_file(db_bytes, db_target, args.force)?;
        audit_log(&format!(
            "restore wrote SQLite store target={} size={}",
            db_target.display(),
            db_bytes.len()
        ));
    }

    audit_log(&format!(
        "restore completed input={} outdir={}",
        args.input.display(),
        args.outdir.display()
    ));
    Ok(())
}

/// Write the archived `data.db` bytes to the live `SQLite` `target` path.
///
/// Safety:
/// - An existing target is preserved as `<target>.bak` before being replaced.
/// - Without `force`, an existing target is refused (no clobber).
/// - Stale `-wal`/`-shm` sidecars next to the target are removed so the
///   restored file is the authoritative state.
/// - The write is atomic (temp file in the same dir → rename).
fn restore_sqlite_file(data: &[u8], target: &Path, force: bool) -> Result<()> {
    if target.exists() && !force {
        bail!(
            "SQLite store {} already exists. Pass --force to replace it \
             (the current file is saved as <path>.bak first).",
            target.display()
        );
    }

    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create parent dir for {}", target.display()))?;
        }
    }

    // Preserve the current store as <target>.bak.
    if target.exists() {
        let bak = {
            let mut p = target.as_os_str().to_os_string();
            p.push(".bak");
            PathBuf::from(p)
        };
        std::fs::rename(target, &bak)
            .with_context(|| format!("back up existing store to {}", bak.display()))?;
    }

    // Atomic write into place.
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let tmp = tempfile::NamedTempFile::new_in(parent).context("create temp file for restore")?;
    std::fs::write(tmp.path(), data)
        .with_context(|| format!("write restored data.db to temp {}", tmp.path().display()))?;
    tmp.persist(target)
        .with_context(|| format!("move restored data.db into place at {}", target.display()))?;

    // Drop stale WAL/SHM sidecars so the restored file is authoritative.
    for suffix in ["-wal", "-shm"] {
        let mut side = target.as_os_str().to_os_string();
        side.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(side));
    }
    Ok(())
}
