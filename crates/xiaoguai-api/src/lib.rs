//! axum HTTP server: REST + SSE.
//!
//! v0.5.5 ships the minimum useful slice — session lifecycle plus an
//! SSE-streamed `POST .../messages` endpoint that drives `ReactAgent`. Auth,
//! RBAC, RLS plumbing, WebSocket fallback, `OpenAPI` generation, the `/v1/mcp`
//! and `/v1/admin` namespaces are tracked in `v0.5.5.1`.

#![forbid(unsafe_code)]

pub mod audit;
pub mod auth;
pub mod convert;
pub mod error;
pub mod eval;
pub mod marketplace;
pub mod mcp_serve;
pub mod rate_limit;
pub mod rbac;
pub mod routes;
pub mod scheduler;
pub mod serve;
pub mod sessions_ext;
pub mod sse;
pub mod state;
pub mod today;
pub mod usage;

pub use audit::{
    AuditEntryView, AuditError, AuditReader, AuditVerifier, StaticAuditReader, StaticAuditVerifier,
    VerifyReport,
};
pub use auth::{Claims, JwtTokenValidator, StubValidator, TokenValidator};
pub use error::{ApiError, ApiResult, UnauthorizedReason};
pub use eval::{
    build_case_yaml, list_suites_in, CaseFromSessionRequest, CaseFromSessionResponse,
    CaseFromSessionSource, EvalService, EvalServiceError, EvalSuiteListItem, RunEvalRequest,
    SessionForCase, StaticCaseFromSessionSource, ToolInvocationRecord,
};
pub use marketplace::{MarketplaceEntry, MarketplaceResponse};
pub use mcp_serve::XiaoguaiMcpServer;
pub use rate_limit::{rate_limit, RateLimiter};
pub use rbac::{method_to_action, path_to_resource, require_authorized};
pub use routes::router;
pub use scheduler::{
    InMemoryWebhookTokenAdmin, NlJobCompileError, NlJobCompiler, RecordingJobUpserter,
    ScheduledJobSummary, ScheduledJobUpsertError, ScheduledJobUpserter, ScheduledJobsReadError,
    ScheduledJobsReader, StaticNlJobCompiler, StaticScheduledJobsReader,
    StaticWebhookTokenValidator, WebhookPushError, WebhookPusher, WebhookTokenAdmin,
    WebhookTokenAdminError, WebhookTokenError, WebhookTokenRecord, WebhookTokenValidator,
};
pub use serve::serve_with_state;
pub use sessions_ext::{SessionForkError, SessionForker};
pub use state::{AppState, CancelRegistry};
pub use today::{StaticTodayReader, TodayError, TodayItem, TodayKind, TodayQuery, TodayReader};
pub use usage::{
    StaticUsageEntry, StaticUsageReader, UsageError, UsageGroupBy, UsageQuery, UsageReader,
    UsageReport, UsageRow,
};
