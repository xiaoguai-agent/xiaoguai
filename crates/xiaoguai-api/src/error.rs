//! API-layer error type. Maps onto HTTP status codes via [`IntoResponse`].
//!
//! Conventions:
//!   - 4xx errors include a stable `code` slug so clients can switch on it
//!     without parsing free-form messages.
//!   - 5xx errors render a generic `internal_error` slug; the original cause
//!     is logged via `tracing` but not surfaced to the response body.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use thiserror::Error;
use xiaoguai_agent::AgentError;
use xiaoguai_storage::repositories::RepoError;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("not found")]
    NotFound,
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("service unavailable: {0}")]
    ServiceUnavailable(String),
    #[error("payload too large: {0}")]
    PayloadTooLarge(String),
    #[error("gateway timeout: {0}")]
    GatewayTimeout(String),
    #[error("internal: {0}")]
    Internal(#[from] anyhow::Error),
    #[error("storage: {0}")]
    Storage(#[from] RepoError),
    #[error("agent: {0}")]
    Agent(#[from] AgentError),
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found", self.to_string()),
            Self::Unauthorized(_) => (StatusCode::UNAUTHORIZED, "unauthorized", self.to_string()),
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request", self.to_string()),
            Self::InvalidRequest(_) => {
                (StatusCode::BAD_REQUEST, "invalid_request", self.to_string())
            }
            Self::Conflict(_) => (StatusCode::CONFLICT, "conflict", self.to_string()),
            Self::ServiceUnavailable(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "service_unavailable",
                self.to_string(),
            ),
            Self::PayloadTooLarge(_) => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                self.to_string(),
            ),
            Self::GatewayTimeout(_) => (
                StatusCode::GATEWAY_TIMEOUT,
                "gateway_timeout",
                self.to_string(),
            ),
            Self::Internal(err) => {
                tracing::error!(?err, "internal api error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "internal error".to_string(),
                )
            }
            Self::Storage(err) => {
                tracing::error!(?err, "storage error in api");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "storage_error",
                    "storage failure".to_string(),
                )
            }
            Self::Agent(err) => {
                tracing::error!(?err, "agent error in api");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "agent_error",
                    "agent failure".to_string(),
                )
            }
        };
        (status, Json(ErrorBody { code, message })).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
