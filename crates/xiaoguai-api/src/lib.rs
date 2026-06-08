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
pub mod hotl;
pub mod identity;
pub mod incidents;
pub mod loops;
pub mod marketplace;
pub mod mcp_serve;
pub mod outcomes;
pub mod routes;
pub mod scheduler;
pub mod serve;
pub mod sessions_ext;
pub mod skill_proposals;
pub mod skills;
pub mod sse;
pub mod state;
pub mod static_ui;
pub mod today;
pub mod turn;
pub mod usage;
pub mod watchers;
pub mod workspaces;

pub use audit::{
    AuditChainExporter, AuditEntryView, AuditError, AuditReader, AuditVerifier, ExportError,
    ExportRequest, StaticAuditChainExporter, StaticAuditReader, StaticAuditVerifier, VerifyReport,
};
pub use auth::{Claims, StaticCredentialValidator, StubValidator, TokenValidator};
pub use error::{ApiError, ApiResult, UnauthorizedReason};
pub use eval::{
    build_case_yaml, list_suites_in, CaseFromSessionRequest, CaseFromSessionResponse,
    CaseFromSessionSource, EvalService, EvalServiceError, EvalSuiteListItem, RunEvalRequest,
    SessionForCase, StaticCaseFromSessionSource, ToolInvocationRecord,
};
pub use hotl::{
    CreateHotlPolicyRequest, HotlEnforcer, HotlPolicy, HotlPolicyStore, HotlPolicyStoreError,
    HotlVerdictResult, InMemoryHotlPolicyStore, StaticHotlEnforcer,
};
pub use incidents::{
    ActionItem, DatadogSource, ImNotification, Incident, IncidentSource, NormalizeError, PrDraft,
    RcaDraft, SentrySource, Severity, TimelineEntry,
};
pub use loops::{CancelLoopError, CreateLoopError, CreateLoopParams, LoopController};
pub use marketplace::{MarketplaceEntry, MarketplaceResponse};
pub use mcp_serve::XiaoguaiMcpServer;
pub use outcomes::{
    InMemoryOutcomeRecorder, InMemoryOutcomesBackend, OutcomeKind, OutcomeWriter, OutcomesApiError,
    OutcomesReader, OutcomesSummaryResponse, OutcomesTimeseriesResponse, RecordOutcomeRequest,
    RecordOutcomeResponse,
};
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
pub use skill_proposals::{ApproveRequest, ListProposalsQuery, ProposalRowResponse, RejectRequest};
pub use skills::{
    CatalogFile, InMemorySkillPackRepository, InstalledPackRow, KnobSchema, PackRequires,
    SkillPackEntry, SkillPackError, SkillPackRepository,
};
pub use state::{AppState, CancelRegistry, TurnGuard};
pub use today::{StaticTodayReader, TodayError, TodayItem, TodayKind, TodayQuery, TodayReader};
pub use turn::{run_turn, TurnCompletion, TurnError, TurnHandle, TurnInput};
pub use usage::{
    StaticUsageEntry, StaticUsageReader, UsageError, UsageGroupBy, UsageQuery, UsageReader,
    UsageReport, UsageRow,
};
pub use watchers::{
    StaticWatcherIntrospector, WatcherError, WatcherInfo, WatcherIntrospector, WatcherSourceType,
    WatcherStatus,
};
pub use workspaces::{
    CreateWorkspaceRequest, InMemoryWorkspaceRepository, UpdateWorkspaceRequest, Workspace,
    WorkspaceError, WorkspaceRepository,
};
