//! Error types for the memory subsystem.

use thiserror::Error;

pub type MemoryResult<T> = Result<T, MemoryError>;

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("memory not found: {0}")]
    NotFound(uuid::Uuid),

    #[error("unknown memory kind: {0:?}")]
    UnknownKind(String),

    #[error("embedding error: {0}")]
    Embedding(String),

    #[error("database error: {0}")]
    Database(String),

    #[error("serialisation error: {0}")]
    Serialisation(#[from] serde_json::Error),
}

#[cfg(feature = "pg")]
impl From<sqlx::Error> for MemoryError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e.to_string())
    }
}
