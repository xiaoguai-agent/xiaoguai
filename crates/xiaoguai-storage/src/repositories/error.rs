//! Shared error type for all repository operations.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RepoError {
    #[error("not found")]
    NotFound,

    #[error("duplicate key: {0}")]
    DuplicateKey(String),

    #[error("foreign key violation: {0}")]
    ForeignKey(String),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// Encryption-at-rest failure: a configured key was malformed, or sealing
    /// a secret field failed. Surfaced loudly rather than silently writing
    /// cleartext.
    #[error("encryption error: {0}")]
    Encryption(String),

    /// v1.1.2 — a repo trait method was called on an impl that doesn't
    /// support it. Used by the default `SessionRepository::fork` body
    /// so test mocks compile without an explicit override; only the
    /// production `SqliteSessionRepository` implements the real copy.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

pub type RepoResult<T> = Result<T, RepoError>;

impl RepoError {
    /// Classify an `sqlx::Error` into a higher-level repo error.
    #[must_use]
    pub fn from_sqlx(err: sqlx::Error) -> Self {
        if let sqlx::Error::Database(ref db_err) = err {
            let code = db_err
                .code()
                .map(std::borrow::Cow::into_owned)
                .unwrap_or_default();
            let msg = db_err.message();
            // SQLite extended result codes for constraint violations.
            match code.as_str() {
                "2067" | "1555" => return Self::DuplicateKey(msg.to_string()),
                "787" => return Self::ForeignKey(msg.to_string()),
                _ => {}
            }
            // Message-text fallback (builds that report only primary code 19).
            if msg.contains("UNIQUE constraint failed") {
                return Self::DuplicateKey(msg.to_string());
            }
            if msg.contains("FOREIGN KEY constraint failed") {
                return Self::ForeignKey(msg.to_string());
            }
        }
        if matches!(err, sqlx::Error::RowNotFound) {
            return Self::NotFound;
        }
        Self::Database(err)
    }
}
