//! axum HTTP server: REST + SSE.
//!
//! v0.5.5 ships the minimum useful slice — session lifecycle plus an
//! SSE-streamed `POST .../messages` endpoint that drives `ReactAgent`. Auth,
//! RBAC, RLS plumbing, WebSocket fallback, `OpenAPI` generation, the `/v1/mcp`
//! and `/v1/admin` namespaces are tracked in `v0.5.5.1`.

#![forbid(unsafe_code)]

pub mod auth;
pub mod convert;
pub mod error;
pub mod routes;
pub mod serve;
pub mod sse;
pub mod state;

pub use auth::{Claims, JwtTokenValidator, StubValidator, TokenValidator};
pub use error::{ApiError, ApiResult};
pub use routes::router;
pub use serve::serve_with_state;
pub use state::{AppState, CancelRegistry};
