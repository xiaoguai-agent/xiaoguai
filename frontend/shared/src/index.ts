/**
 * @xiaoguai/shared — types + API client shared between chat-ui and admin-ui.
 *
 * The types mirror the wire shapes published by `xiaoguai-api` (see
 * `crates/xiaoguai-api/src/routes/sessions.rs` and `.../mcp.rs`). When the
 * Rust crate adds a field, mirror it here.
 */

export const PACKAGE_VERSION = '0.4.0';

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
  tenant_id: string;
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

/** v0.6.3 — directory entry served by `GET /v1/admin/tenants`. */
export interface TenantResponse {
  id: string;
  name: string;
  display_name: string;
  status: 'active' | 'suspended' | 'archived';
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
  | { type: 'error'; message: string };

// ---- Client --------------------------------------------------------------

export interface ApiClientOptions {
  baseUrl: string;
  token?: string;
  fetchImpl?: typeof fetch;
}

export class ApiError extends Error {
  constructor(public readonly status: number, public readonly code: string, message: string) {
    super(message);
    this.name = 'ApiError';
  }
}

export class XiaoguaiClient {
  private readonly baseUrl: string;
  private readonly token?: string;
  private readonly fetchImpl: typeof fetch;

  constructor(opts: ApiClientOptions) {
    this.baseUrl = opts.baseUrl.replace(/\/+$/, '');
    this.token = opts.token;
    this.fetchImpl = opts.fetchImpl ?? fetch;
  }

  private headers(): Record<string, string> {
    const h: Record<string, string> = { 'content-type': 'application/json' };
    if (this.token) {
      h['authorization'] = `Bearer ${this.token}`;
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

  /** v0.6.3 — admin directory of tenants. Requires `system_admin` when
   *  RBAC is on. */
  listTenants(opts?: { limit?: number; offset?: number }): Promise<TenantResponse[]> {
    const params = new URLSearchParams();
    if (opts?.limit !== undefined) params.set('limit', String(opts.limit));
    if (opts?.offset !== undefined) params.set('offset', String(opts.offset));
    const qs = params.toString();
    return this.request<TenantResponse[]>(
      'GET',
      `/v1/admin/tenants${qs ? `?${qs}` : ''}`,
    );
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
  ): () => void {
    const controller = new AbortController();
    void (async () => {
      try {
        const resp = await this.fetchImpl(
          `${this.baseUrl}/v1/sessions/${encodeURIComponent(sessionId)}/messages`,
          {
            method: 'POST',
            headers: this.headers(),
            body: JSON.stringify(body),
            signal: controller.signal,
          },
        );
        if (!resp.ok || !resp.body) {
          onError?.(new ApiError(resp.status, 'http_error', `HTTP ${resp.status}`));
          return;
        }
        const reader = resp.body.getReader();
        const decoder = new TextDecoder('utf-8');
        let buf = '';
        for (;;) {
          const { value, done } = await reader.read();
          if (done) break;
          buf += decoder.decode(value, { stream: true });
          let idx: number;
          while ((idx = buf.indexOf('\n\n')) !== -1) {
            const chunk = buf.slice(0, idx);
            buf = buf.slice(idx + 2);
            const parsed = parseSseChunk(chunk);
            if (parsed) onEvent(parsed);
          }
        }
      } catch (err) {
        if ((err as Error).name !== 'AbortError') {
          onError?.(err as Error);
        }
      }
    })();
    return () => controller.abort();
  }
}

function parseSseChunk(chunk: string): AgentEvent | null {
  let event = '';
  let data = '';
  for (const line of chunk.split('\n')) {
    if (line.startsWith('event:')) {
      event = line.slice(6).trim();
    } else if (line.startsWith('data:')) {
      data += line.slice(5).trim();
    }
  }
  if (!data) return null;
  try {
    const parsed = JSON.parse(data) as AgentEvent;
    if (event && (parsed as { type: string }).type !== event) {
      return { ...(parsed as object), type: event } as unknown as AgentEvent;
    }
    return parsed;
  } catch {
    return null;
  }
}
