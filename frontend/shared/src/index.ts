/**
 * @xiaoguai/shared — types + API client shared between chat-ui and admin-ui.
 *
 * The types mirror the wire shapes published by `xiaoguai-api` (see
 * `crates/xiaoguai-api/src/routes/sessions.rs` and `.../mcp.rs`). When the
 * Rust crate adds a field, mirror it here.
 */

export const PACKAGE_VERSION = '0.2.0';

// ---- Wire types ----------------------------------------------------------

export type SessionStatus = 'active' | 'archived';

export interface SessionResponse {
  id: string;
  tenant_id: string;
  user_id: string;
  title: string | null;
  model: string;
  status: SessionStatus;
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

export type MessageRole = 'system' | 'user' | 'assistant' | 'tool';

export type ContentBlock =
  | { type: 'text'; text: string }
  | { type: 'tool_call'; tool_call_id: string; name: string; arguments: unknown }
  | { type: 'tool_result'; tool_call_id: string; output: unknown; is_error: boolean };

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

  listMcpServers(): Promise<McpServerResponse[]> {
    return this.request<McpServerResponse[]>('GET', '/v1/mcp/servers');
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
