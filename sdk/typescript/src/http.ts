/**
 * Minimal fetch wrapper — no axios, works in Node.js 18+ and modern browsers.
 *
 * Responsibilities:
 * - Attach Authorization header
 * - JSON serialize request bodies
 * - JSON parse response bodies
 * - Map HTTP errors to the XiaoguaiError hierarchy
 * - Honour AbortSignal and custom timeout
 * - Allow injecting a custom fetch implementation (for testing)
 */

import { throwForStatus } from "./errors.js";

export type FetchFn = typeof fetch;

export interface RequestOptions {
  method: "GET" | "POST" | "DELETE" | "PATCH" | "PUT";
  path: string;
  params?: Record<string, string | number | boolean | undefined | null>;
  body?: unknown;
  signal?: AbortSignal;
}

export interface HttpClientConfig {
  baseUrl: string;
  token?: string;
  timeoutMs: number;
  fetch: FetchFn;
}

/** Build a query string from params, omitting undefined/null values. */
function buildQuery(
  params?: Record<string, string | number | boolean | undefined | null>,
): string {
  if (!params) return "";
  const entries = Object.entries(params).filter(
    ([, v]) => v !== undefined && v !== null,
  );
  if (entries.length === 0) return "";
  const qs = entries
    .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(String(v))}`)
    .join("&");
  return `?${qs}`;
}

/** Parse response body as JSON; fall back to plain text. */
async function parseBody(res: Response): Promise<unknown> {
  const ct = res.headers.get("content-type") ?? "";
  if (ct.includes("application/json") || ct.includes("json")) {
    try {
      return await res.json();
    } catch {
      // fall through to text
    }
  }
  const text = await res.text();
  return text ? { error: text } : {};
}

/** Flatten response headers into a plain object for error construction. */
function headersToRecord(headers: Headers): Record<string, string> {
  const out: Record<string, string> = {};
  headers.forEach((value, key) => {
    out[key] = value;
  });
  return out;
}

export class HttpClient {
  private readonly config: HttpClientConfig;

  constructor(config: HttpClientConfig) {
    this.config = config;
  }

  async request<T>(opts: RequestOptions): Promise<T> {
    const { baseUrl, token, timeoutMs, fetch: fetchFn } = this.config;

    const url =
      baseUrl.replace(/\/$/, "") + opts.path + buildQuery(opts.params);

    const headers: Record<string, string> = {
      "Accept": "application/json",
    };
    if (token) {
      headers["Authorization"] = `Bearer ${token}`;
    }
    if (opts.body !== undefined) {
      headers["Content-Type"] = "application/json";
    }

    // Combine caller's AbortSignal with our timeout signal.
    const timeoutController = new AbortController();
    const timeoutId = setTimeout(
      () => timeoutController.abort(new Error(`Request timed out after ${timeoutMs}ms`)),
      timeoutMs,
    );

    // Link external signal (if provided) to our abort controller.
    let externalAbortListener: (() => void) | undefined;
    if (opts.signal) {
      if (opts.signal.aborted) {
        clearTimeout(timeoutId);
        timeoutController.abort(opts.signal.reason);
      } else {
        externalAbortListener = () => timeoutController.abort(opts.signal!.reason);
        opts.signal.addEventListener("abort", externalAbortListener);
      }
    }

    try {
      const res = await fetchFn(url, {
        method: opts.method,
        headers,
        body: opts.body !== undefined ? JSON.stringify(opts.body) : undefined,
        signal: timeoutController.signal,
      });

      const body = await parseBody(res);
      throwForStatus(res.status, body, headersToRecord(res.headers));
      return body as T;
    } finally {
      clearTimeout(timeoutId);
      if (opts.signal && externalAbortListener) {
        opts.signal.removeEventListener("abort", externalAbortListener);
      }
    }
  }

  get<T>(
    path: string,
    params?: Record<string, string | number | boolean | undefined | null>,
    signal?: AbortSignal,
  ): Promise<T> {
    return this.request<T>({ method: "GET", path, params, signal });
  }

  post<T>(path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
    return this.request<T>({ method: "POST", path, body, signal });
  }

  delete<T>(path: string, signal?: AbortSignal): Promise<T> {
    return this.request<T>({ method: "DELETE", path, signal });
  }
}
