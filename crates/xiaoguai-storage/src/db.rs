//! Embedded SQLite connection + migration helpers (DEC-033 single-user pivot).
//!
//! One file, one writer, no replicas. The store lives at `~/.xiaoguai/data.db`
//! (or `$XDG_DATA_HOME/xiaoguai/data.db` when `XDG_DATA_HOME` is set). WAL mode
//! keeps concurrent readers from blocking the single writer; `foreign_keys` is
//! enabled so the schema's `REFERENCES` clauses are enforced at runtime.

use std::{
    path::PathBuf,
    str::FromStr,
    time::Duration,
};

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

/// Resolve the on-disk path of the single-user store.
///
/// `$XDG_DATA_HOME/xiaoguai/data.db` when `XDG_DATA_HOME` is set and non-empty,
/// otherwise `~/.xiaoguai/data.db`. Falls back to a relative `./data.db` only if
/// neither `XDG_DATA_HOME` nor a home directory can be determined.
#[must_use]
pub fn default_db_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.trim().is_empty() {
            return PathBuf::from(xdg).join("xiaoguai").join("data.db");
        }
    }
    if let Some(home) = home_dir() {
        return home.join(".xiaoguai").join("data.db");
    }
    PathBuf::from("data.db")
}

/// Minimal home-directory lookup without pulling in an extra crate.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Turn a configured `database.url` into a concrete SQLite file path.
///
/// Accepts a bare filesystem path, a `sqlite://…` / `sqlite:…` URL, or an empty
/// string / the literal `"default"` (both resolve to [`default_db_path`]).
fn resolve_path(url: &str) -> PathBuf {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed == "default" {
        return default_db_path();
    }
    let stripped = trimmed
        .strip_prefix("sqlite://")
        .or_else(|| trimmed.strip_prefix("sqlite:"))
        .unwrap_or(trimmed);
    if stripped.is_empty() || stripped == ":memory:" {
        return default_db_path();
    }
    PathBuf::from(stripped)
}

/// Open (creating if missing) the single-user SQLite pool.
///
/// `url` is the configured `database.url`; pass an empty string to use the
/// default store path. `max_connections` caps the pool — SQLite is a single
/// writer, so a small pool (reads share, writes serialise) is plenty.
///
/// # Errors
///
/// Returns an error if the parent directory cannot be created or the database
/// file cannot be opened.
pub async fn connect(url: &str, max_connections: u32) -> anyhow::Result<SqlitePool> {
    let path = resolve_path(url);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
        .unwrap_or_else(|_| SqliteConnectOptions::new().filename(&path))
        .filename(&path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));

    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections.max(1))
        .connect_with(opts)
        .await?;
    Ok(pool)
}

/// Run all embedded migrations against `pool`.
///
/// # Errors
///
/// Returns an error if any migration fails to apply.
pub async fn migrate(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}
