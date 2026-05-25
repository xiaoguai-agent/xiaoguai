//! Error type for persona repository operations.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PersonaError {
    #[error("persona not found")]
    NotFound,

    #[error("duplicate persona name for tenant: {0}")]
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
            let code = db_err
                .code()
                .map(std::borrow::Cow::into_owned)
                .unwrap_or_default();
            match code.as_str() {
                // unique_violation
                "23505" => return Self::DuplicateName(db_err.message().to_string()),
                // foreign_key_violation
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
