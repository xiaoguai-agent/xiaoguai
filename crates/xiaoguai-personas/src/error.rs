//! Error type for persona repository operations.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PersonaError {
    #[error("persona not found")]
    NotFound,

    #[error("duplicate persona name: {0}")]
    DuplicateName(String),

    #[error("persona is archived and cannot be attached to new sessions")]
    Archived,

    #[error("foreign key violation: {0}")]
    ForeignKey(String),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

pub type PersonaResult<T> = Result<T, PersonaError>;

impl PersonaError {
    /// Classify an `sqlx::Error` into the appropriate `PersonaError` variant.
    #[must_use]
    pub fn from_sqlx(err: sqlx::Error) -> Self {
        if let sqlx::Error::Database(ref db_err) = err {
            // #283: match on sqlx's driver-normalised `kind()` instead of raw
            // SQLSTATE strings. The previous `"23505"`/`"23503"` arms only
            // ever matched Postgres; the production backend is SQLite, whose
            // native extended codes (`"2067"`/`"787"`) fell through to the
            // generic `Database` arm — surfacing duplicate names as HTTP 500
            // instead of the documented 409 CONFLICT.
            match db_err.kind() {
                sqlx::error::ErrorKind::UniqueViolation => {
                    return Self::DuplicateName(db_err.message().to_string());
                }
                sqlx::error::ErrorKind::ForeignKeyViolation => {
                    return Self::ForeignKey(db_err.message().to_string());
                }
                // `ErrorKind` is #[non_exhaustive]; everything else stays a
                // generic database error below.
                _ => {}
            }
        }
        if matches!(err, sqlx::Error::RowNotFound) {
            return Self::NotFound;
        }
        Self::Database(err)
    }
}
