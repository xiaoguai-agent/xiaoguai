//! Shared error type for all repository operations.

use thiserror::Error;
use uuid::Uuid;

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

    /// v1.1.2 — a repo trait method was called on an impl that doesn't
    /// support it. Used by the default `SessionRepository::fork` body
    /// so test mocks compile without an explicit override; only the
    /// production `PgSessionRepository` implements the real copy.
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// sprint-14 S14-2: a `supersede_policy` attempt found the prior row
    /// no longer at the head of its revision chain. `current_head_id` is
    /// the active head id at the time of the check (or the prior id
    /// itself when the prior was deactivated without supersede).
    /// The admin API surfaces this as HTTP 409 with the head id in the
    /// response body so the operator can rebase and retry.
    #[error("stale revision; current head is {current_head_id}")]
    StaleRevision { current_head_id: Uuid },
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
            // Postgres SQLSTATE codes
            match code.as_str() {
                "23505" => return Self::DuplicateKey(db_err.message().to_string()),
                "23503" => return Self::ForeignKey(db_err.message().to_string()),
                _ => {}
            }
        }
        if matches!(err, sqlx::Error::RowNotFound) {
            return Self::NotFound;
        }
        Self::Database(err)
    }
}
