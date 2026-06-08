/**
 * @xiaoguai/shared — types + API client shared between chat-ui and admin-ui.
 *
 * The types mirror the wire shapes published by `xiaoguai-api` (see
 * `crates/xiaoguai-api/src/routes/sessions.rs` and `.../mcp.rs`). When the
 * Rust crate adds a field, mirror it here.
 */

export const PACKAGE_VERSION = '0.4.0';

// ---- v1.4 Memory subsystem (ADR-0019) — shipped when xiaoguai-memory lands -
export type {
  MemoryType,
  MemoryRecord,
  ListMemoriesQuery,
  ListMemoriesResponse,
  CreateMemoryRequest,
  UpdateMemoryRequest,
  RecallEntry,
  RecallTraceResponse,
  SimilarMemory,
  FindSimilarMemoriesResponse,
} from './memory';

// ---- Wire types ----------------------------------------------------------

export type SessionStatus = 'active' | 'archived';

export interface SessionResponse {
  id: string;
  tenant_id: string;
  user_id: string;
  title: string | null;
  model: string;
  status: SessionStatus;
  /**
   * v1.1.2 — populated when the row was created via
   * `POST /v1/sessions/:id/fork`. Omitted (undefined) on top-level rows.
   */
  parent_session_id?: string;
  /** v1.1.2 — companion to {@link parent_session_id}. */
  forked_from_message_id?: string;
}

export interface CreateSessionRequest {
  user_id: string;
  /** DEC-033: optional — the backend defaults it to the single owner. */
  tenant_id?: string;
  model: string;
  title?: string;
}

export interface SendMessageRequest {
  content: string;
  model?: string;
}

/**
 * v1.1.2 — request body for `POST /v1/sessions/:id/fork`. The handler
 * clones the parent session, copies every message with `created_at <=`
 * the cutoff into the new session, and returns the new
 * {@link SessionResponse}.
 */
export interface ForkSessionRequest {
  from_message_id: string;
  title?: string;
}

export type MessageRole = 'system' | 'user' | 'assistant' | 'tool';

export type ContentBlock =
  | { type: 'text'; text: string }
  | { type: 'tool_call'; tool_call_id: string; name: string; arguments: unknown }
  | { type: 'tool_result'; tool_call_id: string; output: unknown; is_error: boolean }
  /**
   * v0.9.3 — RAG citation. Renders as a click-to-source chip next to
   * the assistant turn. `span` is 1-indexed `[start, end]` line
   * numbers; `(0, 0)` means "no anchor known, link to whole document".
   * `score` is in `[0, 1]` — used for chip opacity + sort order.
   */
  | {
      type: 'citation';
      source_uri: string;
      span: [number, number];
      score: number;
      preview: string;
      collection_id: string;
    };

export interface Message {
  id: string;
  session_id: string;
  role: MessageRole;
  content: ContentBlock[];
  created_at: string;
}

export interface McpServerResponse {
  id: string;
  name: string;
  version: string;
  transport: 'stdio' | 'sse' | 'http';
  command: string | null;
  args: string[];
  env_keys: string[];
  endpoint: string | null;
  tenant_id: string | null;
}

/** v0.6.4 — HMAC-chained audit row served by `GET /v1/admin/audit`. */
export interface AuditEntryView {
  id: number;
  ts: string;
  tenant_id: string;
  actor: string;
  action: string;
  resource: string | null;
  details: unknown;
  /** Lowercase hex, 64 chars. */
  prev_hmac: string;
  /** Lowercase hex, 64 chars. */
  hmac: string;
}

/** Query knobs accepted by `GET /v1/admin/audit`. */
export interface ListAuditQuery {
  tenant_id: string;
  limit?: number;
  /** RFC 3339, inclusive lower bound. */
  since?: string;
  /** RFC 3339, inclusive upper bound. */
  until?: string;
}

/**
 * v1.8.x (sprint-11 S11-1a) — wire shape for `POST /v1/audit/exports`.
 *
 * Backend (`crates/xiaoguai-api/src/routes/audit_exports.rs`) returns the
 * binary export body directly with `Content-Type` set from `format`. There
 * is no async + SSE progress path — this is a single round-trip.
 */
export interface CreateAuditExportRequest {
  tenant_id: string;
  framework: 'soc2' | 'gdpr' | 'hipaa';
  /** Defaults to `"json"` on the backend. PDF currently returns 501. */
  format?: 'json' | 'csv' | 'pdf';
  /** RFC 3339 inclusive lower bound. */
  from: string;
  /** RFC 3339 inclusive upper bound. */
  to: string;
}

/** Resolved download for an audit export. */
export interface AuditExportBlob {
  blob: Blob;
  /** Parsed from `Content-Disposition`, falls back to a synthesised name. */
  filename: string;
  contentType: string;
}

// ---- v0.11.1 — audit-first console (Today endpoint) --------------------

/**
 * Discriminated union returned by `GET /v1/admin/today`. The console
 * renders these as a single timeline (chat / IM / scheduled), sorted by
 * `ts` desc server-side.
 */
export type TodayItem =
  | {
      kind: 'chat';
      ts: string;
      session_id: string;
      tenant_id: string;
      user_id: string;
      started_at: string;
      last_message_preview: string | null;
      message_count: number;
      tool_count: number;
    }
  | {
      kind: 'im';
      ts: string;
      session_id: string;
      tenant_id: string;
      provider: string;
      chat_id: string;
      started_at: string;
      last_message_preview: string | null;
      message_count: number;
    }
  | {
      kind: 'scheduled';
      ts: string;
      job_id: string;
      tenant_id: string | null;
      run_id: number;
      attempt: number;
      status: string;
      fired_at: string;
      output_preview: string | null;
      error_message: string | null;
      /** Populated only on proactive fires (v0.10.2). */
      reason?: string;
    };

export type TodayKind = 'chat' | 'im' | 'scheduled';

export interface ListTodayQuery {
  limit?: number;
  /** RFC 3339, inclusive lower bound on `ts`. */
  since?: string;
  kind?: TodayKind;
}

// ---- v1.1.1 — token usage aggregation -----------------------------------

export type UsageGroupBy = 'day' | 'provider' | 'model';

export interface UsageQuery {
  tenant_id?: string;
  /** RFC 3339, inclusive lower bound on the underlying `ts`. */
  since?: string;
  /** RFC 3339, inclusive upper bound on the underlying `ts`. */
  until?: string;
  /** Defaults to `day` server-side. */
  group_by?: UsageGroupBy;
}

export interface UsageRow {
  /** Bucket key. `day` → `YYYY-MM-DD`; otherwise the provider/model name. */
  bucket: string;
  /** u64 server-side; JSON numbers — caller must tolerate `> Number.MAX_SAFE_INTEGER`
   *  rounding for very large deployments. */
  input_tokens: number;
  output_tokens: number;
  /** `null` until per-provider cost rates are wired (v1.1.1 deferral). */
  cost_cents: number | null;
}

export interface UsageReport {
  rows: UsageRow[];
  total_input_tokens: number;
  total_output_tokens: number;
  /** `null` until per-provider cost rates are wired (v1.1.1 deferral). */
  cost_cents: number | null;
}

// ---- v0.11.2 — eval pane endpoints ------------------------------------

/** Suite list-item returned by `GET /v1/admin/eval/suites`. */
export interface EvalSuiteListItem {
  name: string;
  path: string;
  /** Number of `.eval.yaml` cases under `path`. `null` for single-file suites. */
  case_count: number | null;
}

export interface RunEvalRequest {
  suite_name: string;
  /** Optional override; defaults to `<suites_dir>/<suite_name>` server-side. */
  cases_dir?: string;
}

export type EvalCaseStatus = 'pass' | 'fail';

export interface EvalResult {
  case_id: string;
  status: EvalCaseStatus;
  /** Populated only when `status = 'fail'`. */
  reasons?: string[];
  transcript_len: number;
  duration_ms: number;
}

/** Mirror of `xiaoguai_eval::EvalReport` JSON shape. */
export interface EvalReport {
  suite: string;
  started_at: string;
  finished_at: string;
  results: EvalResult[];
  /** `[0, 1]`. */
  pass_rate: number;
}

export interface CaseFromSessionRequest {
  session_id: string;
}

export interface CaseFromSessionResponse {
  case_yaml: string;
  suggested_filename: string;
  case_id: string;
  tool_invocation_count: number;
}

/** v0.9.4 — curated MCP marketplace entry. */
export interface MarketplaceEntry {
  slug: string;
  name: string;
  description: string;
  category: string;
  transport: 'stdio' | 'sse' | 'http';
  version: string;
  command?: string | null;
  args?: string[];
  endpoint?: string | null;
  env_keys?: string[];
  source_url?: string | null;
}

export interface MarketplaceResponse {
  version: number;
  entries: MarketplaceEntry[];
}

export interface InstallMarketplaceRequest {
  slug: string;
  tenant_id?: string;
}

export interface InstallMarketplaceResponse {
  id: string;
  slug: string;
  name: string;
}

// ---- v0.12.x.1 Scheduler pane -------------------------------------------

/** Mirror of `xiaoguai_api::scheduler::ScheduledJobSummary`. */
export interface ScheduledJobSummary {
  id: string;
  tenant_id: string | null;
  name: string;
  trigger_summary: string;
  enabled: boolean;
  last_fire_at: string | null;
  next_fire_at: string | null;
}

/** Mirror of `xiaoguai_api::scheduler::WebhookTokenRecord`. */
export interface WebhookToken {
  token: string;
  tenant_id: string;
  route_id: string;
  created_at: string;
  last_used_at?: string | null;
}

export interface CompileScheduledJobRequest {
  description: string;
  tenant_id?: string;
}

export interface CompileScheduledJobResponse {
  /** Fully-populated ScheduledJob JSON; pasted into `upsertScheduledJob`. */
  suggested_job: unknown;
  /** One-line human-readable explanation of the compiled job. */
  rationale: string;
}

// ---- v1.2.4 — outcome telemetry -----------------------------------------

/**
 * One of the well-known outcome kinds accepted by `POST /v1/outcomes`.
 * `'custom'` is allowed for operator-defined categories.
 */
export type OutcomeKind =
  | 'revenue_usd'
  | 'cost_saved_usd'
  | 'hours_saved'
  | 'deals_closed'
  | 'tickets_resolved'
  | 'custom';

/** Body for `POST /v1/outcomes`. */
export interface RecordOutcomeRequest {
  tenant_id: string;
  session_id?: string | null;
  agent_name: string;
  kind: string;
  value: number;
  unit?: string | null;
  description?: string | null;
  metadata?: unknown;
}

export interface RecordOutcomeResponse {
  ok: boolean;
}

/**
 * v1.3.x — raw outcome record returned by `GET /v1/outcomes`.
 * Mirrors the `agent_outcomes` row / `OutcomeRecord` Rust struct.
 */
export interface OutcomeRecord {
  tenant_id: string;
  session_id: string | null;
  agent_name: string;
  kind: string;
  value: number;
  unit: string | null;
  description: string | null;
  attributed_at: string; // ISO-8601
  metadata: unknown;
}

/** Query knobs accepted by `GET /v1/outcomes`. */
export interface ListOutcomesQuery {
  /** Optional under the single-user pivot — the backend defaults the owner. */
  tenant_id?: string;
  range?: OutcomesRange;
  kind?: string;
}

/** Aggregate stats for a single outcome kind. */
export interface OutcomeAggregate {
  sum: number;
  count: number;
  avg: number;
}

/** `GET /v1/outcomes/summary` response. */
export interface OutcomesSummaryResponse {
  tenant_id: string;
  range: string;
  summary: {
    by_kind: Record<string, OutcomeAggregate>;
  };
}

/** One daily bucket in `GET /v1/outcomes/timeseries`. */
export interface OutcomeDay {
  date: string;
  kind: string;
  sum: number;
  count: number;
}

/** `GET /v1/outcomes/timeseries` response. */
export interface OutcomesTimeseriesResponse {
  tenant_id: string;
  range: string;
  days: OutcomeDay[];
}

export type OutcomesRange = '24h' | '7d' | '30d';

// ---- v1.3.x — skill pack types -----------------------------------------

/**
 * One installed skill pack as returned by `GET /v1/skills/installed`.
 * The `activation_status` is always `"pending"` until the runtime loader
 * is wired (planned for a future release).
 */
export interface InstalledSkillPackResponse {
  id: string;
  pack_id: string;
  name: string;
  version: string;
  description: string | null;
  /** Agents declared by this pack. Empty until the loader parses pack.yaml. */
  agents: string[];
  /** Inbound adapter types declared (e.g. "http", "slack"). */
  inbound_adapters: string[];
  /** Output types declared (e.g. "telegram", "email"). */
  outputs: string[];
  /** ISO-8601 timestamp when this record was created. */
  recorded_at: string;
  /** Always "pending" — loader activation is not yet wired. */
  activation_status: 'pending';
}

/** Body for `POST /v1/skills/install`. */
export interface InstallSkillPackRequest {
  /** The pack identifier (e.g. "community/web-monitor@1.0.0"). */
  pack_id: string;
  /** Optional display name override. */
  name?: string;
}

export interface InstallSkillPackResponse {
  id: string;
  pack_id: string;
  name: string;
  activation_status: 'pending';
}

// ---- v1.3.x — HotL policy types -----------------------------------------

/**
 * One row in `hotl_policies` as returned by `GET /v1/hotl/policies` and
 * `POST /v1/hotl/policies`.
 */
export interface HotlPolicy {
  id: string;
  tenant_id: string;
  /** Action category (e.g. `"llm_call"`, `"email_send"`, `"webhook_invoke"`). */
  scope: string;
  /** Rolling window width in seconds. Must be > 0. */
  window_seconds: number;
  /** Maximum invocation count within the window. `null` = no count limit. */
  max_count: number | null;
  /** Maximum cumulative USD cost within the window. `null` = no cost limit. */
  max_usd: number | null;
  /** Escalation destination (IM channel / email). `null` = deny on breach. */
  escalate_to: string | null;
}

/**
 * Body for `POST /v1/hotl/policies` and `PUT /v1/hotl/policies/{id}`.
 * At least one of `max_count` / `max_usd` must be non-null.
 */
export interface HotlPolicyCreateRequest {
  tenant_id: string;
  scope: string;
  window_seconds: number;
  max_count?: number | null;
  max_usd?: number | null;
  escalate_to?: string | null;
}

/** Body for `POST /v1/hotl/check`. */
export interface HotlCheckRequest {
  tenant_id: string;
  scope: string;
  /** Increment to record. Use 1.0 for count budget; pass USD cost for cost budget. */
  amount: number;
}

export type HotlVerdictKind = 'allow' | 'escalate' | 'deny';

/** Result of `POST /v1/hotl/check`. */
export interface HotlVerdict {
  verdict: HotlVerdictKind;
  /** Non-null on `escalate` and `deny`, describing which limit was hit. */
  reason: string | null;
}

// ---- v1.8.x — HotL decision-record types (sprint-11 S11-3a/b) -----------
//
// Mirrors `crates/xiaoguai-api/src/routes/hotl_decisions.rs`. Used by the
// chat-ui inline Approve/Reject buttons (S11-3b) — the decision is recorded
// in `hotl_decisions` and optionally creates a follow-up `HotlPolicy`
// ("Approve & remember" / "Deny & tighten"). The backend's `resumed` field
// is always `false` in 3a.1 (no suspend/resume layer yet); the chat-ui
// therefore clears its pending banner optimistically.

/** Wire verdict for `POST /v1/hotl/decisions` — distinct from `HotlVerdictKind` */
/** which carries the budget-check `escalate` value. */
export type HotlDecisionVerdict = 'allow' | 'deny';

/**
 * Sub-DTO carried inside [`SubmitHotlDecisionRequest`]. Mirrors backend
 * `RaisePolicyRequest`. At least one of `max_count` / `max_usd` must be
 * non-null (validated server-side and pre-validated in the chat-ui).
 */
export interface HotlDecisionRaisePolicy {
  scope: string;
  tool?: string;
  window_seconds: number;
  max_count?: number;
  max_usd?: number;
  escalate_to?: string;
}

/** Body for `POST /v1/hotl/decisions`. */
export interface SubmitHotlDecisionRequest {
  /**
   * Sprint-13 S13-9 rename — backend wire field renamed from `request_id`.
   * The legacy alias was removed in S13-8 (v1.10.0); chat-ui v1.10.0 is
   * incompatible with backend < v1.10.0.
   */
  escalation_id: string;
  verdict: HotlDecisionVerdict;
  decided_by: string;
  raise_policy?: HotlDecisionRaisePolicy;
}

/** `201 Created` body returned by `POST /v1/hotl/decisions`. */
export interface HotlDecisionResponse {
  id: string;
  escalation_id: string;
  verdict: HotlDecisionVerdict;
  /** RFC 3339 timestamp. */
  recorded_at: string;
  /**
   * Always `false` in v1.8.x (S11-3a.1). Reserved for the future
   * `SuspendingHotlGate` work — chat-ui must clear the pending banner
   * optimistically, since no `hotl_resolved` SSE event will arrive.
   */
  resumed: boolean;
  /** Present when `raise_policy` was supplied and the policy create succeeded. */
  policy_created?: HotlPolicy | null;
}

// ---- v1.4 (planned) — Anomaly detector types ----------------------------
// Mirrors the DetectorKind + AnomalySpec types in crates/xiaoguai-anomaly/src/spec.rs.
// REST endpoints are PLANNED; the crate is currently a pure Rust library.

/**
 * Severity of a fired anomaly, inferred from sigma distance.
 *   - 'low'      : 2σ–3σ
 *   - 'medium'   : 3σ–5σ
 *   - 'high'     : >5σ
 */
export type AnomalySeverity = 'low' | 'medium' | 'high';

/**
 * One fired anomaly detection event returned by GET /v1/anomaly/detections.
 * Mirrors the runtime output of xiaoguai-anomaly detector evaluation.
 */
export interface AnomalyDetection {
  /** Unique event ID. */
  id: string;
  /** Detector (spec) ID — matches AnomalyDetectorConfig.id. */
  detector_id: string;
  /** RFC 3339 timestamp when the anomaly was detected. */
  fired_at: string;
  severity: AnomalySeverity;
  /** KPI series key / label. */
  series_key: string;
  /** Observed value that triggered the alert. */
  value: number;
  /** Threshold value that was breached (e.g. μ ± n·σ). */
  threshold: number;
  /** Whether the operator has marked this as a false positive. */
  is_false_positive: boolean;
}

/** Response envelope for GET /v1/anomaly/detections. */
export interface AnomalyDetectionListResponse {
  detections: AnomalyDetection[];
  total: number;
}

/** Query parameters for listAnomalyDetections. */
export interface ListAnomalyDetectionsQuery {
  detector_id?: string;
  severity?: AnomalySeverity;
  /** RFC 3339 inclusive lower bound. */
  since?: string;
  /** RFC 3339 inclusive upper bound. */
  until?: string;
  /** Defaults to 50. */
  limit?: number;
}

/**
 * Discriminated union for detector algorithm params.
 * Mirrors DetectorKind in spec.rs.
 */
export type AnomalyDetectorKind =
  | {
      kind: 'z_score';
      sigma_threshold: number;
      min_count: number;
    }
  | {
      kind: 'ewma';
      alpha: number;
      sigma_threshold: number;
      min_count: number;
    };

/**
 * Full detector configuration returned by GET /v1/anomaly/detectors/:id.
 * Corresponds to AnomalySpec in spec.rs plus runtime-mutable tuning fields.
 */
export interface AnomalyDetectorConfig {
  id: string;
  kpi_query: string;
  /** Rolling window in seconds. */
  window_secs: number;
  detector: AnomalyDetectorKind;
  /** Cooldown between alerts in seconds. */
  cool_off_secs: number;
}

/**
 * Partial update payload for PATCH /v1/anomaly/detectors/:id.
 * All fields are optional — only provided fields are updated.
 */
export interface AnomalyDetectorPatch {
  detector?: AnomalyDetectorKind;
  window_secs?: number;
  cool_off_secs?: number;
}

/** Body for POST /v1/anomaly/feedback. */
export interface AnomalyFeedbackRequest {
  /** The detection event ID to mark as false positive. */
  detection_id: string;
  /** true = false positive, false = revoke a previous FP mark. */
  is_false_positive: boolean;
  /** Optional operator note. */
  note?: string;
}

export interface AnomalyFeedbackResponse {
  ok: boolean;
}

/** Aggregated fire-rate bucket for the 14-day trend sparkline. */
export interface AnomalyFireRateBucket {
  /** YYYY-MM-DD date label. */
  date: string;
  /** Number of detections on this date. */
  count: number;
}

// ---- v1.4.0 — Kanban board (task queue) ---------------------------------

/**
 * Six-column Kanban board matching the Hermes Desktop layout.
 * Ships in v1.4; graceful 404 fallback in the admin-ui.
 */
export type TaskColumn = 'triage' | 'todo' | 'ready' | 'running' | 'blocked' | 'done';

export type TaskPriority = 'low' | 'medium' | 'high' | 'critical';

/** Wire shape returned by `GET /v1/tasks` and `GET /v1/tasks/:id`. */
export interface TaskCard {
  id: string;
  board_id: string;
  title: string;
  description: string | null;
  column: TaskColumn;
  priority: TaskPriority;
  assignee: string | null;
  created_at: string;
  updated_at: string;
  deps: string[];
}

/** Board record returned by `GET /v1/tasks/boards`. */
export interface Board {
  id: string;
  name: string;
  description: string | null;
  created_at: string;
}

/** One state-transition entry returned by `GET /v1/tasks/:id/history`. */
export interface TaskHistoryEntry {
  ts: string;
  from_column: TaskColumn | null;
  to_column: TaskColumn;
  actor: string | null;
  note: string | null;
}

/** Body for `POST /v1/tasks`. */
export interface CreateTaskRequest {
  board_id: string;
  title: string;
  description?: string | null;
  column?: TaskColumn;
  priority?: TaskPriority;
  assignee?: string | null;
}

/** Body for `PATCH /v1/tasks/:id/column`. */
export interface UpdateTaskColumnRequest {
  column: TaskColumn;
}

/** Body for `POST /v1/tasks/:id/block`. */
export interface BlockTaskRequest {
  reason: string;
}

/** Body for `POST /v1/tasks/boards`. */
export interface CreateBoardRequest {
  name: string;
  description?: string | null;
}

// ---- v1.3.x — HotL (Human-on-the-Loop) policy --------------------------

/**
 * Marker injected into the agent event stream (as `type: 'hotl_pending'`)
 * when the agent loop is suspended on a HotL decision. The UI must surface
 * a non-dismissible banner until the matching `hotl_resolved` event arrives
 * or the session ends.
 *
 * Wire shape per [`api-contract.md`](../../xiaoguai-agent-design/docs/api-contract.md)
 * §2.6.3 (sprint-12 S12-2). The v1.3.x shape (`escalation_id` + `reason`)
 * has been retired — the backend never emits the old shape now that
 * `SuspendingHotlGate` is the only producer of this event.
 */
export interface HotlPendingEvent {
  type: 'hotl_pending';
  /** Suspended escalation id; pairs 1:1 with `HotlResolvedEvent.escalation_id`. */
  escalation_id: string;
  /** Tool name whose dispatch is suspended (e.g. `execute_python`). */
  tool: string;
  /** Policy-driven redaction of the tool arguments (opaque JSON shape). */
  args_redacted: unknown;
  /** Policy scope that matched, e.g. `tool_call.execute_python`. */
  scope: string;
  /** RFC 3339; server-side decision deadline (default: now + 24h). */
  expires_at: string;
}

/**
 * Emitted by `xiaoguai-agent` after the suspended decision resolves —
 * either via operator verdict (`POST /v1/hotl/decisions`) or the
 * server-side timeout. Frontend keys the `<HotlBanner>` on `escalation_id`
 * and clears the matching pending state on receipt.
 *
 * Wire shape per `api-contract.md` §2.6.3. `decided_by` is `null` when
 * `verdict === 'timeout'`. The `request_id` legacy field was retired in
 * sprint-13 S13-8 (backend) / S13-9 (chat-ui).
 */
export interface HotlResolvedEvent {
  type: 'hotl_resolved';
  escalation_id: string;
  verdict: 'allow' | 'deny' | 'timeout';
  decided_by: string | null;
  recorded_at: string;
}

// ---- v1.3.x — Session-scoped outcome events -----------------------------

/**
 * Lightweight event emitted by the runtime when an outcome is recorded
 * during the current session. Included in the agent event stream alongside
 * `text_delta`, `tool_call_*`, etc.
 */
export interface OutcomeRecordedEvent {
  type: 'outcome_recorded';
  kind: string;
  value: number;
  unit: string | null;
  description: string | null;
  /** RFC 3339 timestamp. */
  ts: string;
}

/**
 * Extended API response for `GET /v1/outcomes/summary?session_id=<id>`.
 * The base `OutcomesSummaryResponse` is tenant-scoped; this variant adds
 * per-session fields and a recent-events list.
 */
export interface SessionOutcomesSummary {
  session_id: string;
  tenant_id: string;
  /** Counts + aggregates by outcome kind. */
  by_kind: Record<string, { count: number; sum: number; unit: string | null }>;
  /** The 5 most recent outcome events recorded in this session, newest first. */
  recent: Array<{
    kind: string;
    value: number;
    unit: string | null;
    description: string | null;
    ts: string;
  }>;
}

// ---- v1.3.x — AI disclosure banner (EU AI Act Art. 50(1)) ---------------

/**
 * Per-tenant configuration for the AI disclosure banner.
 *
 * Backend endpoint (required follow-up, separate task):
 *   GET /v1/tenants/:id/config  →  field `ai_disclosure_banner: AiDisclosureConfig`
 *
 * Until the backend field is wired, the client falls back to the defaults
 * documented below (enabled=true, dismissible=true, no text_override, no link).
 */
export interface AiDisclosureConfig {
  /** When false the banner is hidden entirely. Default: true. */
  enabled: boolean;
  /**
   * Custom locale text for the banner body. When omitted the built-in
   * translated strings from the i18n locale files are used.
   */
  text_override?: string | null;
  /**
   * When false the dismiss button is hidden and the banner is always
   * visible — intended for regulated tenants (e.g. financial services
   * subject to stricter AI Act obligations). Default: true.
   */
  dismissible: boolean;
  /**
   * URL to the operator's full transparency note / AI disclosure page.
   * When provided a "Learn more" link is rendered at the end of the banner.
   */
  link_to_disclosure?: string | null;
}

const AI_DISCLOSURE_CONFIG_DEFAULTS: AiDisclosureConfig = {
  enabled: true,
  dismissible: true,
};

/**
 * Resolve the AI disclosure banner config.
 *
 * DEC-033: the per-tenant `GET /v1/tenants/:id/config` endpoint was removed
 * with multi-tenancy, so this now simply returns the built-in defaults
 * (banner enabled + dismissible). The signature is kept for call-site
 * stability; the args are accepted but unused. Operators who want to
 * override the banner can do so via the optional `config` prop on
 * `<AiDisclosureBanner>` instead.
 */
export async function getAiDisclosureConfig(
  _tenantId?: string,
  _opts?: { baseUrl?: string; fetchImpl?: typeof fetch },
): Promise<AiDisclosureConfig> {
  return AI_DISCLOSURE_CONFIG_DEFAULTS;
}

// ---- v1.3.x — watch event indicators ------------------------------------

/** Status values a watcher can be in. */
export type WatcherStatus = 'running' | 'paused' | 'error';

/** Source type that produced the watcher (schedule, webhook, manual, etc.). */
export type WatcherSourceType = 'schedule' | 'webhook' | 'manual' | string;

/**
 * A single watcher row returned by `GET /v1/watchers?session_id=<id>`.
 *
 * NOTE: /v1/watchers is not yet implemented server-side (separate task).
 * The client handles 404/503 gracefully by returning [].
 */
export interface WatcherInfo {
  id: string;
  /** Human-readable label, e.g. "Nightly report watcher". */
  name: string;
  /** Where this watcher originated. */
  source_type: WatcherSourceType;
  /** RFC 3339; null when never fired. */
  last_fired_at: string | null;
  /** Current lifecycle status. */
  status: WatcherStatus;
  /** Cron or interval expression; null for manual/webhook-driven watchers. */
  schedule?: string | null;
}

// ---- v1.8.0 (sprint-10b S10b-2) — Persona CRUD ---------------------------

/**
 * A named role profile that shapes agent behaviour within a tenant.
 *
 * Mirrors `xiaoguai_personas::Persona` (crates/xiaoguai-personas/src/model.rs).
 * Field names match the Rust DTO 1:1.
 */
export interface Persona {
  id: string;
  tenant_id: string;
  name: string;
  /** Injected as the leading system message in every chat turn. */
  system_prompt: string;
  /** Optional model override. `null` = use the session / global default. */
  default_model: string | null;
  /** `null` = unrestricted (all tools). `[]` = no tools allowed. */
  tool_allowlist: string[] | null;
  /** Opaque escalation tier label for HOTL integration (e.g. "L1"). */
  escalation_tier: string | null;
  created_at: string;
  /** Soft-deleted personas cannot be attached to new sessions. */
  archived: boolean;
}

export interface CreatePersonaRequest {
  tenant_id: string;
  name: string;
  system_prompt?: string;
  default_model?: string | null;
  tool_allowlist?: string[] | null;
  escalation_tier?: string | null;
}

/**
 * Optional updates. Omitted fields retain their value.
 * `tool_allowlist: null` here means "clear → unrestricted"; the Rust
 * model uses `Option<Option<Vec<String>>>` to disambiguate "do not
 * change" from "clear", which JS can't model directly — pass `undefined`
 * (don't include the key) for "do not change", `null` for "clear".
 */
export interface UpdatePersonaRequest {
  name?: string;
  system_prompt?: string;
  default_model?: string | null;
  tool_allowlist?: string[] | null;
  escalation_tier?: string | null;
}

// ---- v1.8.0 (sprint-10b S10b-3) — Skill Proposals ------------------------

/**
 * The shape an agent-authored skill manifest takes when the admin pane
 * (or any other reviewer) gets it back from the API. Mirrors
 * `xiaoguai_tasks::skill_author::SkillManifest`
 * (crates/xiaoguai-tasks/src/skill_author.rs).
 *
 * Field names match the Rust DTO 1:1. The schema is intentionally tight —
 * `additionalProperties: false` on the propose side, so the manifest only
 * ever carries these five fields. Reviewers can trust the shape.
 */
export interface SkillManifest {
  /** Skill identifier (kebab-case, unique per tenant). */
  name: string;
  /** One-sentence description for the catalog. */
  description: string;
  /** SemVer string the agent supplied at propose time. */
  version: string;
  /** The instructions the skill will receive when the runtime loads it. */
  system_prompt: string;
  /** Tools the skill is allowed to call. Empty `[]` means "no tools". */
  tool_allowlist: string[];
}

/** Lifecycle states a proposal moves through. */
export type SkillProposalStatus =
  | 'pending'
  | 'approved'
  | 'rejected'
  | 'installed';

/**
 * One row from `GET /v1/skills/proposals`. Mirrors
 * `xiaoguai_api::skill_proposals::ProposalRowResponse`.
 *
 * `proposed_by` is the agent identity (typically `agent:<persona-id>`);
 * `decided_by` is the human admin who clicked Approve/Reject.
 */
export interface SkillProposal {
  id: string;
  tenant_id: string;
  proposed_by: string;
  manifest: SkillManifest;
  status: SkillProposalStatus;
  /** Set when `status === 'rejected'`. */
  reason: string | null;
  created_at: string;
  decided_at: string | null;
  decided_by: string | null;
}

/** Query options for `GET /v1/skills/proposals`. */
export interface ListSkillProposalsQuery {
  /** Required — backend rejects the call without it. */
  tenant_id: string;
  /** Defaults to "all" when omitted. */
  status?: SkillProposalStatus;
}

/**
 * Body for `POST /v1/skills/proposals/:id/approve`.
 *
 * Backend gap (S10b-3, DEC-014): the Rust handler does NOT accept an
 * optional reviewer `comment`; only `decided_by` is recognised. If you
 * want to capture rationale, log it to the audit sink separately or extend
 * the backend handler.
 */
export interface ApproveSkillProposalRequest {
  /** Identity of the approver. Logged to `skill_proposals.decided_by`. */
  decided_by: string;
}

/** Body for `POST /v1/skills/proposals/:id/reject`. */
export interface RejectSkillProposalRequest {
  decided_by: string;
  /** Non-empty rationale shown back to the agent. */
  reason: string;
}

// ---- /loop — session-scoped recurring agent turns (DEC-039 / LLD-LOOP-001) -
//
// Mirrors `crates/xiaoguai-api/src/routes/loops.rs`. A loop re-runs a prompt
// on a session at a cadence until a budget (ticks / ttl / tokens) is hit or an
// operator cancels it.

/** Lifecycle status of a loop. Mirrors `LoopStatus::as_str` (storage crate). */
export type LoopStatus =
  | 'active'
  | 'paused'
  | 'budget_exhausted'
  | 'done'
  | 'cancelled'
  | 'failed';

/** How a loop chooses its next-tick delay. Mirrors `PacingKind::as_str`. */
export type LoopPacingKind = 'fixed' | 'dynamic';

/**
 * Body for `POST /v1/loops`. Only `session_id` + `prompt` are required; the
 * backend fills the defaults the chat-ui surfaces in its confirmation bubble
 * (interval 300s, max 50 ticks, ttl 24h).
 */
export interface CreateLoopRequest {
  session_id: string;
  prompt: string;
  interval_secs?: number;
  max_ticks?: number;
  ttl_secs?: number;
  /** L3 Part B — let the agent pace the loop via `loop_next_tick`. */
  dynamic_pacing?: boolean;
  min_interval_secs?: number;
  max_interval_secs?: number;
  /** L3 Part C — token budget; `0` = unlimited, omitted = backend default. */
  max_total_tokens?: number;
}

/**
 * A loop row as returned by `POST` / `GET` / `DELETE /v1/loops`. Mirrors the
 * Rust `LoopResponse` (crates/xiaoguai-api/src/routes/loops.rs) 1:1.
 */
export interface LoopResponse {
  id: string;
  session_id: string;
  prompt: string;
  pacing_kind: LoopPacingKind;
  interval_secs: number;
  min_interval_secs: number;
  max_interval_secs: number;
  max_ticks: number;
  ttl_secs: number;
  max_total_tokens: number;
  status: LoopStatus;
  created_by: string;
  created_at: string;
  expires_at: string;
  next_tick_at: string;
  ticks_run: number;
  consecutive_failures: number;
  /** Present only when the loop has recorded a tick error. */
  last_error?: string;
}

// ---- Agent event stream --------------------------------------------------

export type AgentEvent =
  | { type: 'text_delta'; delta: string }
  | { type: 'tool_call_started'; id: string; name: string; arguments: unknown }
  | {
      type: 'tool_call_finished';
      id: string;
      name: string;
      ok: boolean;
      error?: string | null;
      output_text?: string | null;
    }
  | { type: 'iteration_completed'; iteration: number }
  | { type: 'done'; stop_reason: 'completed' | 'max_iterations' | 'cancelled' }
  | { type: 'error'; message: string }
  | HotlPendingEvent
  | HotlResolvedEvent
  | OutcomeRecordedEvent;

// ---- Client (memory type aliases for method signatures) -------------------

import type {
  ListMemoriesQuery,
  ListMemoriesResponse,
  CreateMemoryRequest,
  UpdateMemoryRequest,
  MemoryRecord,
  RecallTraceResponse,
  FindSimilarMemoriesResponse,
} from './memory';

// ---- Client --------------------------------------------------------------

/**
 * Single-owner HTTP Basic credentials (DEC-033). The backend has no OIDC /
 * bearer tokens / tenants — access is gated by one configured
 * username + password. When the backend runs open (no credential set) this
 * is omitted and no `Authorization` header is sent.
 */
export interface BasicCredentials {
  username: string;
  password: string;
}

export interface ApiClientOptions {
  baseUrl: string;
  /** HTTP Basic credentials. Omit for an open (localhost-dev) backend. */
  basicAuth?: BasicCredentials;
  /**
   * Invoked when any request returns 401. The UI registers this to surface a
   * login prompt. Also settable post-construction via
   * {@link XiaoguaiClient.setUnauthorizedHandler}.
   */
  onUnauthorized?: () => void;
  fetchImpl?: typeof fetch;
}

/** Base64-encode a UTF-8 string for the HTTP Basic `Authorization` header. */
function encodeBasic(username: string, password: string): string {
  const raw = `${username}:${password}`;
  if (typeof btoa === 'function') {
    // btoa is latin1-only; round-trip through UTF-8 so non-ASCII creds work.
    return btoa(unescape(encodeURIComponent(raw)));
  }
  // Node / SSR fallback.
  return Buffer.from(raw, 'utf-8').toString('base64');
}

export class ApiError extends Error {
  constructor(public readonly status: number, public readonly code: string, message: string) {
    super(message);
    this.name = 'ApiError';
  }
}

/**
 * Optional callbacks for {@link XiaoguaiClient.sendMessage} retry behaviour.
 * Per LLD-CHAT-UI-001 §4.7 / §4.7.1.
 */
export interface SendMessageOptions {
  /**
   * Invoked just before each retry sleeps. `attempt` is 1-based (1 = first
   * retry after the initial failure). `delayMs` is the backoff about to be
   * slept. ChatPage uses this to mount the reconnect banner.
   */
  onReconnect?: (attempt: number, delayMs: number) => void;
  /**
   * Maximum number of retries after the first failure. Defaults to 5 →
   * backoff sequence 1+2+4+8+16 = 31 s before giving up.
   */
  maxRetries?: number;
}

/**
 * Generate an Idempotency-Key for sendMessage retries. Format mirrors a
 * UUID-ish 16-byte hex stream so backends matching RFC 4122 dedup keys can
 * accept it. crypto.randomUUID() is preferred when available; otherwise
 * fall back to Math.random hex (non-cryptographic — acceptable for a
 * client-side dedup hint, not a security boundary).
 */
function generateIdempotencyKey(): string {
  const g = globalThis as { crypto?: { randomUUID?: () => string } };
  if (g.crypto?.randomUUID) return g.crypto.randomUUID();
  const hex = (n: number) => Math.floor(Math.random() * n).toString(16);
  return `${hex(0xffffffff)}-${hex(0xffff)}-${hex(0xffff)}-${hex(0xffff)}-${hex(0xffffffff)}${hex(0xffff)}`;
}

/** Provider `kind` values accepted by `POST /v1/admin/providers`. */
export type ProviderKind =
  | 'ollama'
  | 'openai_compat'
  | 'anthropic'
  | 'gemini'
  | 'bedrock'
  | 'azure_openai'
  | 'mistral'
  | 'groq'
  | 'minimax';

/** A configured LLM provider as returned by the admin API. The stored API key
 *  is never serialised — only `has_api_key`. */
export interface LlmProviderView {
  id: string;
  name: string;
  kind: string;
  endpoint: string;
  models: string[];
  default_for_models: string[];
  fallback_order: number;
  api_key_env: string | null;
  has_api_key: boolean;
}

/** Request body for `POST /v1/admin/providers`. Supply `api_key` for a hosted
 *  API, or leave both keys blank for a local URL (Ollama / OpenAI-compatible). */
export interface CreateProviderRequest {
  name: string;
  kind: ProviderKind;
  endpoint: string;
  models?: string[];
  default_for_models?: string[];
  fallback_order?: number;
  api_key?: string;
  api_key_env?: string;
}

/** Exponential backoff schedule for sendMessage retries (ms), capped at 30 s. */
const RECONNECT_BACKOFF_MS: readonly number[] = [1000, 2000, 4000, 8000, 16000];

export class XiaoguaiClient {
  private readonly baseUrl: string;
  private basicAuth?: BasicCredentials;
  private onUnauthorized?: () => void;
  private readonly fetchImpl: typeof fetch;

  constructor(opts: ApiClientOptions) {
    this.baseUrl = opts.baseUrl.replace(/\/+$/, '');
    this.basicAuth = opts.basicAuth;
    this.onUnauthorized = opts.onUnauthorized;
    this.fetchImpl = opts.fetchImpl ?? fetch;
  }

  /** Set (or clear, with `undefined`) the HTTP Basic credentials at runtime —
   *  e.g. after the user submits the login form. */
  setBasicAuth(creds: BasicCredentials | undefined): void {
    this.basicAuth = creds;
  }

  /** Register (or clear) the 401 handler that opens the login prompt. */
  setUnauthorizedHandler(fn: (() => void) | undefined): void {
    this.onUnauthorized = fn;
  }

  private headers(): Record<string, string> {
    const h: Record<string, string> = { 'content-type': 'application/json' };
    if (this.basicAuth) {
      h['authorization'] = `Basic ${encodeBasic(this.basicAuth.username, this.basicAuth.password)}`;
    }
    return h;
  }

  private async request<T>(method: string, path: string, body?: unknown): Promise<T> {
    const resp = await this.fetchImpl(`${this.baseUrl}${path}`, {
      method,
      headers: this.headers(),
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
    if (!resp.ok) {
      // 401 → the owner credential is missing/wrong; let the UI prompt login.
      if (resp.status === 401) {
        this.onUnauthorized?.();
      }
      let code = 'http_error';
      let message = `HTTP ${resp.status}`;
      try {
        const parsed = (await resp.json()) as { code?: string; message?: string };
        if (parsed.code) code = parsed.code;
        if (parsed.message) message = parsed.message;
      } catch {
        // body wasn't JSON; keep defaults.
      }
      throw new ApiError(resp.status, code, message);
    }
    return (await resp.json()) as T;
  }

  async healthz(): Promise<string> {
    const resp = await this.fetchImpl(`${this.baseUrl}/healthz`);
    return await resp.text();
  }

  createSession(req: CreateSessionRequest): Promise<SessionResponse> {
    return this.request<SessionResponse>('POST', '/v1/sessions', req);
  }

  getSession(id: string): Promise<SessionResponse> {
    return this.request<SessionResponse>('GET', `/v1/sessions/${encodeURIComponent(id)}`);
  }

  listMessages(sessionId: string): Promise<Message[]> {
    return this.request<Message[]>('GET', `/v1/sessions/${encodeURIComponent(sessionId)}/messages`);
  }

  cancel(sessionId: string): Promise<{ cancelled: boolean }> {
    return this.request('POST', `/v1/sessions/${encodeURIComponent(sessionId)}/cancel`, {});
  }

  /**
   * v1.1.2 — branch a session at a given message boundary. Returns the
   * newly-created child session. UI flow: click "Branch from here" on
   * an assistant bubble → call this with `from_message_id = that
   * message's id` → `window.open` the returned `id`.
   */
  forkSession(sessionId: string, req: ForkSessionRequest): Promise<SessionResponse> {
    return this.request<SessionResponse>(
      'POST',
      `/v1/sessions/${encodeURIComponent(sessionId)}/fork`,
      req,
    );
  }

  listMcpServers(): Promise<McpServerResponse[]> {
    return this.request<McpServerResponse[]>('GET', '/v1/mcp/servers');
  }

  /** List configured LLM providers (local URLs + hosted APIs). The stored
   *  API key is never returned — only `has_api_key`. */
  listProviders(): Promise<LlmProviderView[]> {
    return this.request<LlmProviderView[]>('GET', '/v1/admin/providers');
  }

  /** Register a provider — a local model URL (`ollama` / `openai_compat`) or a
   *  hosted API (`minimax`, `openai_compat` for Zhipu/OpenAI/DeepSeek, …).
   *  Takes effect on the next server restart (the router is built at boot). */
  createProvider(req: CreateProviderRequest): Promise<LlmProviderView> {
    return this.request<LlmProviderView>('POST', '/v1/admin/providers', req);
  }

  /** Delete a provider by id. */
  async deleteProvider(id: string): Promise<void> {
    const resp = await this.fetchImpl(
      `${this.baseUrl}/v1/admin/providers/${encodeURIComponent(id)}`,
      { method: 'DELETE', headers: this.headers() },
    );
    if (!resp.ok) {
      throw new ApiError(resp.status, 'http_error', `HTTP ${resp.status}`);
    }
  }

  /** v0.6.4 — HMAC-chained audit rows for a single tenant. */
  listAudit(q: ListAuditQuery): Promise<AuditEntryView[]> {
    const params = new URLSearchParams({ tenant_id: q.tenant_id });
    if (q.limit !== undefined) params.set('limit', String(q.limit));
    if (q.since) params.set('since', q.since);
    if (q.until) params.set('until', q.until);
    return this.request<AuditEntryView[]>('GET', `/v1/admin/audit?${params.toString()}`);
  }

  /**
   * v1.8.x (sprint-11 S11-1a) — compliance export. Posts the window +
   * framework and resolves with the binary blob the backend returns.
   *
   * Goes through `fetchImpl` directly (not `this.request`) because the
   * response body is binary; matches the `ApiError` shape used by
   * `request<T>` for non-2xx responses. On 2xx the filename is parsed
   * from `Content-Disposition`, falling back to a synthesised name keyed
   * off tenant id + ts + format extension.
   */
  async createAuditExport(req: CreateAuditExportRequest): Promise<AuditExportBlob> {
    const format = req.format ?? 'json';
    const resp = await this.fetchImpl(`${this.baseUrl}/v1/audit/exports`, {
      method: 'POST',
      headers: this.headers(),
      body: JSON.stringify({ ...req, format }),
    });
    if (!resp.ok) {
      let code = 'http_error';
      let message = `HTTP ${resp.status}`;
      try {
        const parsed = (await resp.json()) as { code?: string; message?: string };
        if (parsed.code) code = parsed.code;
        if (parsed.message) message = parsed.message;
      } catch {
        // body wasn't JSON; keep defaults.
      }
      throw new ApiError(resp.status, code, message);
    }
    const blob = await resp.blob();
    const contentType =
      resp.headers.get('content-type') ?? blob.type ?? 'application/octet-stream';
    const disposition = resp.headers.get('content-disposition');
    const filename = parseContentDispositionFilename(disposition) ?? defaultExportFilename(req.tenant_id, format);
    return { blob, filename, contentType };
  }

  /**
   * v0.11.1 — composite Today timeline. The console makes this the
   * default landing pane (audit-first, not chat-first).
   */
  listToday(q?: ListTodayQuery): Promise<TodayItem[]> {
    const params = new URLSearchParams();
    if (q?.limit !== undefined) params.set('limit', String(q.limit));
    if (q?.since) params.set('since', q.since);
    if (q?.kind) params.set('kind', q.kind);
    const qs = params.toString();
    return this.request<TodayItem[]>('GET', `/v1/admin/today${qs ? `?${qs}` : ''}`);
  }

  /**
   * v1.1.1 — token-usage aggregation. The admin-ui Usage pane drives
   * this directly; the Today pane uses it for the 24h summary card.
   */
  getUsage(q?: UsageQuery): Promise<UsageReport> {
    const params = new URLSearchParams();
    if (q?.tenant_id) params.set('tenant_id', q.tenant_id);
    if (q?.since) params.set('since', q.since);
    if (q?.until) params.set('until', q.until);
    if (q?.group_by) params.set('group_by', q.group_by);
    const qs = params.toString();
    return this.request<UsageReport>('GET', `/v1/usage${qs ? `?${qs}` : ''}`);
  }

  /** v0.9.4 — curated MCP server catalog. */
  listMarketplace(): Promise<MarketplaceResponse> {
    return this.request<MarketplaceResponse>('GET', '/v1/mcp/marketplace');
  }

  /** v0.11.2 — enumerate suites discoverable under the configured suites_dir. */
  listEvalSuites(): Promise<EvalSuiteListItem[]> {
    return this.request<EvalSuiteListItem[]>('GET', '/v1/admin/eval/suites');
  }

  /** v0.11.2 — run a suite synchronously. Suites cap at 100 cases / 60s. */
  runEvalSuite(req: RunEvalRequest): Promise<EvalReport> {
    return this.request<EvalReport>('POST', '/v1/admin/eval/run', req);
  }

  /** v0.11.2 — convert a prod `sessions.id` into a ready-to-edit case YAML. */
  evalCaseFromSession(
    req: CaseFromSessionRequest,
  ): Promise<CaseFromSessionResponse> {
    return this.request<CaseFromSessionResponse>(
      'POST',
      '/v1/admin/eval/case-from-session',
      req,
    );
  }

  // ---- v0.12.x.1 Scheduler pane ------------------------------------------

  /** Enumerate scheduled jobs for the admin-ui Scheduler pane. */
  listScheduledJobs(opts?: { limit?: number }): Promise<ScheduledJobSummary[]> {
    const params = new URLSearchParams();
    if (opts?.limit !== undefined) params.set('limit', String(opts.limit));
    const qs = params.toString();
    return this.request<ScheduledJobSummary[]>(
      'GET',
      `/v1/admin/scheduler/jobs${qs ? `?${qs}` : ''}`,
    );
  }

  /** Fire a scheduled job out-of-band. Returns 202; run is async. */
  fireScheduledJob(jobId: string): Promise<{ fired: string }> {
    return this.request<{ fired: string }>(
      'POST',
      `/v1/admin/scheduler/jobs/${encodeURIComponent(jobId)}/fire-now`,
    );
  }

  /** Compile a free-form job description into a `ScheduledJob` JSON. */
  compileScheduledJob(
    req: CompileScheduledJobRequest,
  ): Promise<CompileScheduledJobResponse> {
    return this.request<CompileScheduledJobResponse>(
      'POST',
      '/v1/admin/scheduler/jobs/compile',
      req,
    );
  }

  /** Upsert a `ScheduledJob` row (insert or update by id). */
  upsertScheduledJob(job: unknown): Promise<{ id: string }> {
    return this.request<{ id: string }>(
      'POST',
      '/v1/admin/scheduler/jobs',
      job,
    );
  }

  /** List per-tenant webhook tokens. */
  listWebhookTokens(opts?: {
    tenant_id?: string;
    limit?: number;
  }): Promise<WebhookToken[]> {
    const params = new URLSearchParams();
    if (opts?.tenant_id) params.set('tenant_id', opts.tenant_id);
    if (opts?.limit !== undefined) params.set('limit', String(opts.limit));
    const qs = params.toString();
    return this.request<WebhookToken[]>(
      'GET',
      `/v1/admin/scheduler/tokens${qs ? `?${qs}` : ''}`,
    );
  }

  /** Mint a new webhook token bound to `(tenant_id, route_id)`. */
  createWebhookToken(req: {
    tenant_id: string;
    route_id: string;
  }): Promise<WebhookToken> {
    return this.request<WebhookToken>(
      'POST',
      '/v1/admin/scheduler/tokens',
      req,
    );
  }

  /** Revoke (delete) a webhook token. Returns 204; no body. */
  async revokeWebhookToken(token: string): Promise<void> {
    const resp = await this.fetchImpl(
      `${this.baseUrl}/v1/admin/scheduler/tokens/${encodeURIComponent(token)}`,
      { method: 'DELETE', headers: this.headers() },
    );
    if (!resp.ok) {
      throw new ApiError(resp.status, 'http_error', `HTTP ${resp.status}`);
    }
  }

  /** v0.9.4 — one-click install of a marketplace entry. */
  installMarketplace(
    req: InstallMarketplaceRequest,
  ): Promise<InstallMarketplaceResponse> {
    return this.request<InstallMarketplaceResponse>(
      'POST',
      '/v1/mcp/marketplace/install',
      req,
    );
  }

  // ---- v1.3.x — skill pack browser ----------------------------------------

  /** List all recorded (installed) skill packs via `GET /v1/skills/installed`. */
  listInstalledSkillPacks(): Promise<InstalledSkillPackResponse[]> {
    return this.request<InstalledSkillPackResponse[]>('GET', '/v1/skills/installed');
  }

  /**
   * Record a skill pack installation via `POST /v1/skills/install`.
   * Note: the runtime loader is not yet wired; packs are recorded but
   * not activated until a future release.
   */
  installSkillPack(req: InstallSkillPackRequest): Promise<InstallSkillPackResponse> {
    return this.request<InstallSkillPackResponse>('POST', '/v1/skills/install', req);
  }

  // ---- v1.4.0 Kanban board (task queue) ----------------------------------

  /** List all boards. `GET /v1/tasks/boards` */
  listBoards(): Promise<Board[]> {
    return this.request<Board[]>('GET', '/v1/tasks/boards');
  }

  /** Create a new board. `POST /v1/tasks/boards` */
  createBoard(req: CreateBoardRequest): Promise<Board> {
    return this.request<Board>('POST', '/v1/tasks/boards', req);
  }

  /** List tasks on a board, optionally filtered by column. `GET /v1/tasks?board=X` */
  listTasks(opts: { board_id: string; column?: TaskColumn }): Promise<TaskCard[]> {
    const params = new URLSearchParams({ board: opts.board_id });
    if (opts.column) params.set('column', opts.column);
    return this.request<TaskCard[]>('GET', `/v1/tasks?${params.toString()}`);
  }

  /** Create a new task. `POST /v1/tasks` */
  createTask(req: CreateTaskRequest): Promise<TaskCard> {
    return this.request<TaskCard>('POST', '/v1/tasks', req);
  }

  /** Move a task to a different column. `PATCH /v1/tasks/:id/column` */
  updateTaskColumn(taskId: string, req: UpdateTaskColumnRequest): Promise<TaskCard> {
    return this.request<TaskCard>(
      'PATCH',
      `/v1/tasks/${encodeURIComponent(taskId)}/column`,
      req,
    );
  }

  /**
   * Dispatch — moves the next READY task to RUNNING.
   * `POST /v1/tasks/dispatch?board=X`
   */
  dispatchTask(boardId: string): Promise<TaskCard | null> {
    return this.request<TaskCard | null>(
      'POST',
      `/v1/tasks/dispatch?board=${encodeURIComponent(boardId)}`,
    );
  }

  /** Block a task with a reason. `POST /v1/tasks/:id/block` */
  blockTask(taskId: string, req: BlockTaskRequest): Promise<TaskCard> {
    return this.request<TaskCard>(
      'POST',
      `/v1/tasks/${encodeURIComponent(taskId)}/block`,
      req,
    );
  }

  /** Fetch state-transition history for a task. `GET /v1/tasks/:id/history` */
  getTaskHistory(taskId: string): Promise<TaskHistoryEntry[]> {
    return this.request<TaskHistoryEntry[]>(
      'GET',
      `/v1/tasks/${encodeURIComponent(taskId)}/history`,
    );
  }

  // ---- v1.2.4 Outcomes --------------------------------------------------

  /** Record a business outcome attribution. */
  recordOutcome(req: RecordOutcomeRequest): Promise<RecordOutcomeResponse> {
    return this.request<RecordOutcomeResponse>('POST', '/v1/outcomes', req);
  }

  /** ROI summary cards — aggregated by kind. */
  getOutcomesSummary(opts: {
    /** Optional under the single-user pivot — the backend defaults the owner. */
    tenant_id?: string;
    range?: OutcomesRange;
  } = {}): Promise<OutcomesSummaryResponse> {
    const params = new URLSearchParams();
    if (opts.tenant_id) params.set('tenant_id', opts.tenant_id);
    if (opts.range) params.set('range', opts.range);
    const qs = params.toString();
    return this.request<OutcomesSummaryResponse>(
      'GET',
      `/v1/outcomes/summary${qs ? `?${qs}` : ''}`,
    );
  }

  // ---- v1.4 Memory (ADR-0019) — 404 until xiaoguai-memory ships -----------

  /**
   * List memories with optional filters. Returns 404 if the memory
   * subsystem (`xiaoguai-memory` crate, task #155) is not yet deployed.
   */
  listMemories(q?: ListMemoriesQuery): Promise<ListMemoriesResponse> {
    const params = new URLSearchParams();
    if (q?.type) params.set('type', q.type);
    if (q?.tenant_id) params.set('tenant_id', q.tenant_id);
    if (q?.agent_id) params.set('agent_id', q.agent_id);
    if (q?.tag) params.set('tag', q.tag);
    if (q?.since) params.set('since', q.since);
    if (q?.until) params.set('until', q.until);
    if (q?.limit !== undefined) params.set('limit', String(q.limit));
    if (q?.offset !== undefined) params.set('offset', String(q.offset));
    const qs = params.toString();
    return this.request<ListMemoriesResponse>('GET', `/v1/memory${qs ? `?${qs}` : ''}`);
  }

  /** Create a new memory record. */
  createMemory(req: CreateMemoryRequest): Promise<MemoryRecord> {
    return this.request<MemoryRecord>('POST', '/v1/memory', req);
  }

  /** Fetch a single memory by id. */
  getMemory(id: string): Promise<MemoryRecord> {
    return this.request<MemoryRecord>('GET', `/v1/memory/${encodeURIComponent(id)}`);
  }

  /** Update mutable fields (content, tags, ttl) of an existing memory. */
  updateMemory(id: string, req: UpdateMemoryRequest): Promise<MemoryRecord> {
    return this.request<MemoryRecord>('PATCH', `/v1/memory/${encodeURIComponent(id)}`, req);
  }

  /** Delete a memory record by id. Returns 204; no body. */
  async deleteMemory(id: string): Promise<void> {
    const resp = await this.fetchImpl(
      `${this.baseUrl}/v1/memory/${encodeURIComponent(id)}`,
      { method: 'DELETE', headers: this.headers() },
    );
    if (!resp.ok) {
      throw new ApiError(resp.status, 'http_error', `HTTP ${resp.status}`);
    }
  }

  /**
   * Fetch the recall trace for a session or free-form query.
   * Useful for debugging "why did the agent forget X?".
   */
  recallMemoriesForSession(opts: {
    session_id?: string;
    query?: string;
    limit?: number;
  }): Promise<RecallTraceResponse> {
    const params = new URLSearchParams();
    if (opts.session_id) params.set('session_id', opts.session_id);
    if (opts.query) params.set('query', opts.query);
    if (opts.limit !== undefined) params.set('limit', String(opts.limit));
    return this.request<RecallTraceResponse>('GET', `/v1/memory/recall?${params.toString()}`);
  }

  /**
   * Find N nearest neighbors by vector similarity for a given memory.
   * Useful for surfacing duplicate or conflicting memories.
   */
  findSimilarMemories(memoryId: string, opts?: { top_k?: number }): Promise<FindSimilarMemoriesResponse> {
    const params = new URLSearchParams();
    if (opts?.top_k !== undefined) params.set('top_k', String(opts.top_k));
    const qs = params.toString();
    return this.request<FindSimilarMemoriesResponse>(
      'GET',
      `/v1/memory/${encodeURIComponent(memoryId)}/similar${qs ? `?${qs}` : ''}`,
    );
  }

  /**
   * v1.3.x — session-scoped outcome summary polled by `RecentOutcomesPanel`.
   * Calls `GET /v1/outcomes/summary?session_id=<id>`.
   */
  getSessionOutcomesSummary(sessionId: string): Promise<SessionOutcomesSummary> {
    const params = new URLSearchParams({ session_id: sessionId });
    return this.request<SessionOutcomesSummary>(
      'GET',
      `/v1/outcomes/summary?${params.toString()}`,
    );
  }

  /** Daily time-series — bar chart data. */
  getOutcomesTimeseries(opts: {
    /** Optional under the single-user pivot — the backend defaults the owner. */
    tenant_id?: string;
    range?: OutcomesRange;
    kind?: string;
  } = {}): Promise<OutcomesTimeseriesResponse> {
    const params = new URLSearchParams();
    if (opts.tenant_id) params.set('tenant_id', opts.tenant_id);
    if (opts.range) params.set('range', opts.range);
    if (opts.kind) params.set('kind', opts.kind);
    const qs = params.toString();
    return this.request<OutcomesTimeseriesResponse>(
      'GET',
      `/v1/outcomes/timeseries${qs ? `?${qs}` : ''}`,
    );
  }

  // ---- v1.3.x HotL policies -----------------------------------------------

  /**
   * List HOTL policies for a tenant. Returns 503 when `PgHotlPolicyStore`
   * is not yet wired (store bridge pending).
   */
  listHotlPolicies(opts: { tenant_id: string; scope?: string }): Promise<HotlPolicy[]> {
    const params = new URLSearchParams({ tenant_id: opts.tenant_id });
    if (opts.scope) params.set('scope', opts.scope);
    return this.request<HotlPolicy[]>('GET', `/v1/hotl/policies?${params.toString()}`);
  }

  /** Create a new HOTL policy. Returns 201 with the persisted row. */
  createHotlPolicy(req: HotlPolicyCreateRequest): Promise<HotlPolicy> {
    return this.request<HotlPolicy>('POST', '/v1/hotl/policies', req);
  }

  /** Full replacement of a HOTL policy by `id`. */
  updateHotlPolicy(id: string, req: HotlPolicyCreateRequest): Promise<HotlPolicy> {
    return this.request<HotlPolicy>('PUT', `/v1/hotl/policies/${encodeURIComponent(id)}`, req);
  }

  /**
   * Delete a HOTL policy. Returns 204 (no body). Throws ApiError(404) when
   * the id is unknown.
   */
  async deleteHotlPolicy(id: string): Promise<void> {
    const resp = await this.fetchImpl(
      `${this.baseUrl}/v1/hotl/policies/${encodeURIComponent(id)}`,
      { method: 'DELETE', headers: this.headers() },
    );
    if (!resp.ok) {
      let code = 'http_error';
      let message = `HTTP ${resp.status}`;
      try {
        const parsed = (await resp.json()) as { code?: string; message?: string };
        if (parsed.code) code = parsed.code;
        if (parsed.message) message = parsed.message;
      } catch { /* body was not JSON */ }
      throw new ApiError(resp.status, code, message);
    }
  }

  // ---- v1.3.x Watch indicators -----------------------------------------

  /**
   * List active watchers attached to a session.
   *
   * NOTE: /v1/watchers is not yet implemented server-side (separate task).
   * This method gracefully returns [] on 404 or 503.
   */
  async listSessionWatchers(sessionId: string): Promise<WatcherInfo[]> {
    try {
      return await this.request<WatcherInfo[]>(
        'GET',
        `/v1/watchers?session_id=${encodeURIComponent(sessionId)}`,
      );
    } catch (err) {
      // Endpoint not yet exposed — render nothing, not an error banner.
      if (err instanceof ApiError && (err.status === 404 || err.status === 503)) {
        return [];
      }
      throw err;
    }
  }

  /**
   * Check budget for `(tenant_id, scope)` and record the action.
   * Returns `allow` / `escalate` / `deny` verdict.
   */
  checkHotlPolicy(req: HotlCheckRequest): Promise<HotlVerdict> {
    return this.request<HotlVerdict>('POST', '/v1/hotl/check', req);
  }

  /**
   * Record an operator decision against an escalated HOTL request
   * (sprint-11 S11-3a/b). Returns the persisted record and, when
   * `raise_policy` was supplied, the follow-up `HotlPolicy` row.
   *
   * NOTE: `resumed` is always `false` in v1.8.x — the backend records
   * the decision but does not resume any agent loop yet (no
   * suspend/resume layer). The caller (chat-ui HotlBanner) clears its
   * `hotlPending` state optimistically; no `hotl_resolved` SSE event
   * arrives because nothing was suspended.
   */
  submitHotlDecision(
    req: SubmitHotlDecisionRequest,
  ): Promise<HotlDecisionResponse> {
    return this.request<HotlDecisionResponse>('POST', '/v1/hotl/decisions', req);
  }

  // ---- Streaming ----------------------------------------------------------

  /**
   * v1.3.x — raw list of outcome records for a tenant.
   * Backs the List view in the Outcomes browser pane.
   */
  listOutcomes(q: ListOutcomesQuery = {}): Promise<OutcomeRecord[]> {
    const params = new URLSearchParams();
    if (q.tenant_id) params.set('tenant_id', q.tenant_id);
    if (q.range) params.set('range', q.range);
    if (q.kind) params.set('kind', q.kind);
    const qs = params.toString();
    return this.request<OutcomeRecord[]>('GET', `/v1/outcomes${qs ? `?${qs}` : ''}`);
  }

  // ---- v1.4 (planned) — Anomaly detector endpoints -----------------------
  // NOTE: The REST endpoints /v1/anomaly/* are PLANNED but not yet
  // implemented in xiaoguai-api. The xiaoguai-anomaly crate exists as a
  // pure Rust library only. These methods handle 404/503 gracefully so the
  // UI degrades to a placeholder rather than crashing.

  /**
   * List recent anomaly detections.
   * Endpoint: GET /v1/anomaly/detections
   * Status: PLANNED — endpoint may not exist yet. Returns [] on 404/503.
   */
  listAnomalyDetections(
    opts?: ListAnomalyDetectionsQuery,
  ): Promise<AnomalyDetectionListResponse> {
    const params = new URLSearchParams();
    if (opts?.detector_id) params.set('detector_id', opts.detector_id);
    if (opts?.severity) params.set('severity', opts.severity);
    if (opts?.since) params.set('since', opts.since);
    if (opts?.until) params.set('until', opts.until);
    if (opts?.limit !== undefined) params.set('limit', String(opts.limit));
    const qs = params.toString();
    return this.request<AnomalyDetectionListResponse>(
      'GET',
      `/v1/anomaly/detections${qs ? `?${qs}` : ''}`,
    );
  }

  /**
   * Get current config for a single anomaly detector.
   * Endpoint: GET /v1/anomaly/detectors/:id
   * Status: PLANNED — endpoint may not exist yet.
   */
  getAnomalyDetector(detectorId: string): Promise<AnomalyDetectorConfig> {
    return this.request<AnomalyDetectorConfig>(
      'GET',
      `/v1/anomaly/detectors/${encodeURIComponent(detectorId)}`,
    );
  }

  /**
   * Update tuning params for an anomaly detector. HotL-gated on the server
   * (changing detection thresholds affects audit posture).
   * Endpoint: PATCH /v1/anomaly/detectors/:id
   * Status: PLANNED — endpoint may not exist yet.
   */
  updateAnomalyDetector(
    detectorId: string,
    patch: AnomalyDetectorPatch,
  ): Promise<AnomalyDetectorConfig> {
    return this.request<AnomalyDetectorConfig>(
      'PATCH',
      `/v1/anomaly/detectors/${encodeURIComponent(detectorId)}`,
      patch,
    );
  }

  /**
   * Submit a false-positive feedback signal for a detection event.
   * Endpoint: POST /v1/anomaly/feedback
   * Status: PLANNED — endpoint may not exist yet.
   */
  submitAnomalyFeedback(
    req: AnomalyFeedbackRequest,
  ): Promise<AnomalyFeedbackResponse> {
    return this.request<AnomalyFeedbackResponse>('POST', '/v1/anomaly/feedback', req);
  }

  /**
   * Pause a watcher by id. Returns silently when endpoint absent (404/503).
   */
  async pauseWatcher(watcherId: string): Promise<void> {
    try {
      await this.request<unknown>(
        'POST',
        `/v1/watchers/${encodeURIComponent(watcherId)}/pause`,
      );
    } catch (err) {
      if (err instanceof ApiError && (err.status === 404 || err.status === 503)) {
        return;
      }
      throw err;
    }
  }

  /**
   * Resume a paused or error-state watcher.
   * HotL-gated for error-state resume on the server side.
   */
  async resumeWatcher(watcherId: string): Promise<void> {
    try {
      await this.request<unknown>(
        'POST',
        `/v1/watchers/${encodeURIComponent(watcherId)}/resume`,
      );
    } catch (err) {
      if (err instanceof ApiError && (err.status === 404 || err.status === 503)) {
        return;
      }
      throw err;
    }
  }

  // ---- v1.8.0 (sprint-10b S10b-2) — Persona CRUD -----------------------

  /**
   * List active personas for the tenant. The backend requires
   * `tenant_id` as a query parameter; this client method takes the
   * tenant from the caller so the admin-ui can switch tenants.
   */
  listPersonas(tenantId: string): Promise<Persona[]> {
    return this.request<Persona[]>(
      'GET',
      `/v1/personas?tenant_id=${encodeURIComponent(tenantId)}`,
    );
  }

  /** Fetch a single persona by UUID. */
  getPersona(id: string): Promise<Persona> {
    return this.request<Persona>('GET', `/v1/personas/${encodeURIComponent(id)}`);
  }

  /** Create a new persona. */
  createPersona(req: CreatePersonaRequest): Promise<Persona> {
    return this.request<Persona>('POST', '/v1/personas', req);
  }

  /**
   * Update a persona. Note the route uses PATCH (partial update);
   * the older PUT shape from the prompt is not what the Rust router
   * mounts — see crates/xiaoguai-api/src/routes/mod.rs personas mount.
   */
  updatePersona(id: string, req: UpdatePersonaRequest): Promise<Persona> {
    return this.request<Persona>(
      'PATCH',
      `/v1/personas/${encodeURIComponent(id)}`,
      req,
    );
  }

  /**
   * Archive (soft-delete) a persona. Returns silently on success;
   * the backend responds 204 No Content.
   */
  async deletePersona(id: string): Promise<void> {
    const resp = await this.fetchImpl(
      `${this.baseUrl}/v1/personas/${encodeURIComponent(id)}`,
      { method: 'DELETE', headers: this.headers() },
    );
    if (!resp.ok) {
      let code = 'http_error';
      let message = `HTTP ${resp.status}`;
      try {
        const parsed = (await resp.json()) as { code?: string; message?: string };
        if (parsed.code) code = parsed.code;
        if (parsed.message) message = parsed.message;
      } catch {
        // body was not JSON
      }
      throw new ApiError(resp.status, code, message);
    }
  }

  // ---- v1.8.0 (sprint-10b S10b-3) — Skill Proposals -------------------

  /**
   * List agent-authored skill proposals. `tenant_id` is required by the
   * backend; the optional `status` filter defaults to "all" when omitted.
   *
   * The backend returns 503 when the proposal repository is not wired —
   * the caller should render a "feature unavailable" banner in that case.
   */
  listSkillProposals(q: ListSkillProposalsQuery): Promise<SkillProposal[]> {
    const params = new URLSearchParams();
    params.set('tenant_id', q.tenant_id);
    if (q.status) params.set('status', q.status);
    return this.request<SkillProposal[]>(
      'GET',
      `/v1/skills/proposals?${params.toString()}`,
    );
  }

  /**
   * Approve a pending proposal. Returns the updated row (status will be
   * "installed" — the backend writes the YAML manifest to `skills_dir` as
   * part of the transition).
   */
  approveSkillProposal(
    id: string,
    req: ApproveSkillProposalRequest,
  ): Promise<SkillProposal> {
    return this.request<SkillProposal>(
      'POST',
      `/v1/skills/proposals/${encodeURIComponent(id)}/approve`,
      req,
    );
  }

  /** Reject a pending proposal with a mandatory rationale. */
  rejectSkillProposal(
    id: string,
    req: RejectSkillProposalRequest,
  ): Promise<SkillProposal> {
    return this.request<SkillProposal>(
      'POST',
      `/v1/skills/proposals/${encodeURIComponent(id)}/reject`,
      req,
    );
  }

  // ---- /loop — recurring agent turns (DEC-039 / LLD-LOOP-001) -------------

  /**
   * Create + arm a loop on a session. `POST /v1/loops` → 201 with the row.
   * Throws `ApiError(409)` when the session already has a live loop or is
   * archived, `404` for an unknown session, `503` when loops are unwired.
   */
  createLoop(req: CreateLoopRequest): Promise<LoopResponse> {
    return this.request<LoopResponse>('POST', '/v1/loops', req);
  }

  /** List every loop, newest first (terminal rows included). `GET /v1/loops` */
  listLoops(): Promise<LoopResponse[]> {
    return this.request<LoopResponse[]>('GET', '/v1/loops');
  }

  /** Fetch a single loop by id. `GET /v1/loops/:id` */
  getLoop(id: string): Promise<LoopResponse> {
    return this.request<LoopResponse>('GET', `/v1/loops/${encodeURIComponent(id)}`);
  }

  /**
   * Cancel a live loop. `DELETE /v1/loops/:id` → 200 with the terminalised
   * row. Throws `ApiError(409)` when already terminal, `404` when unknown.
   */
  cancelLoop(id: string): Promise<LoopResponse> {
    return this.request<LoopResponse>('DELETE', `/v1/loops/${encodeURIComponent(id)}`);
  }

  /**
   * `POST /v1/sessions/:id/messages` — streams `AgentEvent`s. Each chunk
   * in the SSE response becomes one onEvent call. Returns a function the
   * caller can use to abort the stream.
   */
  sendMessage(
    sessionId: string,
    body: SendMessageRequest,
    onEvent: (ev: AgentEvent) => void,
    onError?: (err: Error) => void,
    opts?: SendMessageOptions,
  ): () => void {
    const controller = new AbortController();
    const maxRetries = opts?.maxRetries ?? RECONNECT_BACKOFF_MS.length;
    // Per plan §3 / DEC-LLD-CHAT-UI-003: generate once, reuse on retries so a
    // dedup-aware backend treats the resend as the same logical request.
    // Behaviour degrades safely (duplicate message) if the backend ignores it.
    const idempotencyKey = generateIdempotencyKey();
    const url = `${this.baseUrl}/v1/sessions/${encodeURIComponent(sessionId)}/messages`;
    const serialized = JSON.stringify(body);

    // Highest SSE `id:` seen so far across all attempts. Echoed back as
    // `Last-Event-ID` on reconnect so a resume-capable backend can pick up
    // from the cursor; today's backend restarts the run, so the chat client
    // also drops the superseded turn to avoid duplicate text (F5).
    let lastEventId: string | null = null;

    const buildHeaders = (attempt: number): Record<string, string> => {
      const h = this.headers();
      // Only send the idempotency header on retries so the happy path stays
      // byte-identical to the previous behaviour. (See sendMessage.test.ts
      // "Idempotency-Key header" case.)
      if (attempt > 0) h['idempotency-key'] = idempotencyKey;
      // Standard SSE resume cursor — only present once an event with an id
      // has been observed (i.e. on a reconnect after partial progress).
      if (lastEventId !== null) h['last-event-id'] = lastEventId;
      return h;
    };

    const sleep = (ms: number): Promise<void> =>
      new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          controller.signal.removeEventListener('abort', onAbort);
          resolve();
        }, ms);
        const onAbort = () => {
          clearTimeout(timer);
          controller.signal.removeEventListener('abort', onAbort);
          const err = new Error('aborted');
          err.name = 'AbortError';
          reject(err);
        };
        if (controller.signal.aborted) {
          onAbort();
        } else {
          controller.signal.addEventListener('abort', onAbort);
        }
      });

    void (async () => {
      const decoder = new TextDecoder('utf-8');
      let buf = '';
      let lastErr: Error | null = null;

      for (let attempt = 0; attempt <= maxRetries; attempt += 1) {
        if (attempt > 0) {
          const idx = Math.min(attempt - 1, RECONNECT_BACKOFF_MS.length - 1);
          const delayMs = Math.min(RECONNECT_BACKOFF_MS[idx]!, 30000);
          try {
            opts?.onReconnect?.(attempt, delayMs);
          } catch {
            // Don't let a callback throw kill the retry loop.
          }
          try {
            await sleep(delayMs);
          } catch {
            // Aborted during backoff — caller cancelled, exit silently.
            return;
          }
        }
        try {
          const resp = await this.fetchImpl(url, {
            method: 'POST',
            headers: buildHeaders(attempt),
            body: serialized,
            signal: controller.signal,
          });
          if (!resp.ok || !resp.body) {
            lastErr = new ApiError(resp.status, 'http_error', `HTTP ${resp.status}`);
            // A 4xx is a client error — the same request will be rejected
            // identically on every retry, so burning the full backoff is
            // pointless. 408 (Request Timeout) and 429 (Too Many Requests)
            // are the standard retryable exceptions; everything else 4xx
            // fails fast via onError. 5xx / missing-body still retry.
            if (
              resp.status >= 400 &&
              resp.status < 500 &&
              resp.status !== 408 &&
              resp.status !== 429
            ) {
              break;
            }
            continue;
          }
          const reader = resp.body.getReader();
          // Drain the stream. A buf carried over from a prior partial attempt
          // is intentionally preserved across iterations so a delta split by
          // a mid-chunk disconnect can be reassembled when the server resumes.
          for (;;) {
            const { value, done } = await reader.read();
            if (done) {
              // Clean EOF — stream ended successfully.
              return;
            }
            buf += decoder.decode(value, { stream: true });
            let idx: number;
            while ((idx = buf.indexOf('\n\n')) !== -1) {
              const chunk = buf.slice(0, idx);
              buf = buf.slice(idx + 2);
              const parsed = parseSseChunk(chunk);
              // Track the resume cursor even for keep-alive / data-less frames.
              if (parsed.id !== null) lastEventId = parsed.id;
              if (parsed.ev) onEvent(parsed.ev);
            }
          }
        } catch (err) {
          if ((err as Error).name === 'AbortError') {
            // Explicit cancel — do not retry.
            return;
          }
          lastErr = err as Error;
          continue;
        }
      }

      if (lastErr) onError?.(lastErr);
    })();

    return () => controller.abort();
  }
}

/**
 * Parse an RFC 6266 `Content-Disposition` header for the `filename` parameter.
 * Handles plain `filename="…"` and the `filename*=UTF-8''…` extended form.
 * Returns `null` when the header is missing or the parameter cannot be
 * extracted — callers should fall back to a synthesised filename.
 */
function parseContentDispositionFilename(header: string | null): string | null {
  if (!header) return null;
  const ext = header.match(/filename\*\s*=\s*[^']*''([^;]+)/i);
  if (ext?.[1]) {
    try {
      return decodeURIComponent(ext[1].trim());
    } catch {
      // fall through to the plain form.
    }
  }
  const quoted = header.match(/filename\s*=\s*"([^"]+)"/i);
  if (quoted?.[1]) return quoted[1];
  const bare = header.match(/filename\s*=\s*([^;]+)/i);
  if (bare?.[1]) return bare[1].trim();
  return null;
}

/** Synthesise a download filename when the backend omits Content-Disposition. */
function defaultExportFilename(tenantId: string, format: string): string {
  const ext = format.toLowerCase();
  const ts = new Date().toISOString().replace(/[:.]/g, '-');
  return `audit-${tenantId}-${ts}.${ext}`;
}

/** One parsed SSE frame: the decoded event plus its `id:` field (if any). */
interface ParsedSseChunk {
  ev: AgentEvent | null;
  /** The SSE `id:` field — a per-stream monotonic sequence, or null. */
  id: string | null;
}

function parseSseChunk(chunk: string): ParsedSseChunk {
  let event = '';
  let data = '';
  let id: string | null = null;
  for (const line of chunk.split('\n')) {
    if (line.startsWith('event:')) {
      event = line.slice(6).trim();
    } else if (line.startsWith('data:')) {
      data += line.slice(5).trim();
    } else if (line.startsWith('id:')) {
      id = line.slice(3).trim();
    }
  }
  if (!data) return { ev: null, id };
  try {
    const parsed = JSON.parse(data) as AgentEvent;
    if (event && (parsed as { type: string }).type !== event) {
      return { ev: { ...(parsed as object), type: event } as unknown as AgentEvent, id };
    }
    return { ev: parsed, id };
  } catch {
    return { ev: null, id };
  }
}
