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
