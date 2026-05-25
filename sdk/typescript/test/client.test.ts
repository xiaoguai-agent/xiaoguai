/**
 * XiaoguaiClient test suite — 25 vitest cases covering:
 * - Happy-path for all implemented endpoints
 * - 4xx error mapping (401, 403, 404, 409, 422, 429)
 * - 5xx error mapping
 * - AbortSignal cancellation
 * - Custom fetch injection
 * - Retry-After header parsing
 * - NotImplementedError for stub methods
 * - Query parameter serialization
 */

import { describe, it, expect, vi } from "vitest";
import {
  XiaoguaiClient,
  XiaoguaiError,
  HttpError,
  AuthError,
  ForbiddenError,
  NotFoundError,
  ConflictError,
  ValidationError,
  RateLimitError,
  ServerError,
  NotImplementedError,
} from "../src/index.js";
import type { HotlPolicy, InstalledSkillPack } from "../src/index.js";

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/** Build a mock fetch that returns a fixed response. */
function mockFetch(status: number, body: unknown, headers: Record<string, string> = {}): typeof fetch {
  return vi.fn(async (_url: RequestInfo | URL, _init?: RequestInit) => {
    const responseHeaders = new Headers({
      "Content-Type": "application/json",
      ...headers,
    });
    return new Response(JSON.stringify(body), {
      status,
      headers: responseHeaders,
    });
  }) as unknown as typeof fetch;
}

/** Capture the URL that was requested. */
function capturingFetch(
  status: number,
  body: unknown,
): { fetch: typeof fetch; lastUrl: () => string } {
  let last = "";
  const fn = vi.fn(async (url: RequestInfo | URL, _init?: RequestInit) => {
    last = String(url);
    return new Response(JSON.stringify(body), {
      status,
      headers: new Headers({ "Content-Type": "application/json" }),
    });
  }) as unknown as typeof fetch;
  return { fetch: fn, lastUrl: () => last };
}

const BASE = "http://localhost:8080";

function client(fetchImpl: typeof fetch): XiaoguaiClient {
  return new XiaoguaiClient({ baseUrl: BASE, token: "test-token", fetch: fetchImpl });
}

// ---------------------------------------------------------------------------
// HotL — happy-path tests
// ---------------------------------------------------------------------------

describe("HotL policies", () => {
  const policyFixture: HotlPolicy = {
    id: "aaaabbbb-0000-0000-0000-000000000001",
    tenant_id: "tenant-1",
    scope: "llm_call",
    window_seconds: 3600,
    max_count: 100,
    max_usd: null,
    escalate_to: "ops@example.com",
  };

  it("listHotlPolicies — returns array on 200", async () => {
    const c = client(mockFetch(200, [policyFixture]));
    const result = await c.listHotlPolicies({ tenant_id: "tenant-1" });
    expect(result).toHaveLength(1);
    expect(result[0]!.scope).toBe("llm_call");
    expect(result[0]!.max_count).toBe(100);
  });

  it("listHotlPolicies — sends scope param in query string", async () => {
    const { fetch: fn, lastUrl } = capturingFetch(200, []);
    const c = client(fn);
    await c.listHotlPolicies({ tenant_id: "tenant-1", scope: "email_send" });
    expect(lastUrl()).toContain("scope=email_send");
    expect(lastUrl()).toContain("tenant_id=tenant-1");
  });

  it("createHotlPolicy — sends POST and returns policy", async () => {
    const fn = mockFetch(201, policyFixture);
    const c = client(fn);
    const result = await c.createHotlPolicy({
      tenant_id: "tenant-1",
      scope: "llm_call",
      window_seconds: 3600,
      max_count: 100,
    });
    expect(result.id).toBe(policyFixture.id);
    const calls = (fn as ReturnType<typeof vi.fn>).mock.calls;
    expect(calls[0]![1]!.method).toBe("POST");
  });

  it("deleteHotlPolicy — resolves void on 204", async () => {
    const c = client(
      vi.fn(async () =>
        new Response(null, { status: 204 }),
      ) as unknown as typeof fetch,
    );
    await expect(c.deleteHotlPolicy("some-id")).resolves.toBeUndefined();
  });

  it("getHotlPolicy — throws NotImplementedError", () => {
    const c = client(mockFetch(200, {}));
    expect(() => c.getHotlPolicy("any-id")).toThrow(NotImplementedError);
  });

  it("updateHotlPolicy — throws NotImplementedError", () => {
    const c = client(mockFetch(200, {}));
    expect(() => c.updateHotlPolicy("any-id", {})).toThrow(NotImplementedError);
  });

  it("checkHotl — throws NotImplementedError", () => {
    const c = client(mockFetch(200, {}));
    expect(() => c.checkHotl("llm_call", 1.0)).toThrow(NotImplementedError);
  });
});

// ---------------------------------------------------------------------------
// Outcomes — happy-path tests
// ---------------------------------------------------------------------------

describe("Outcomes", () => {
  it("recordOutcome — returns true on 201 ok:true", async () => {
    const c = client(mockFetch(201, { ok: true }));
    const ok = await c.recordOutcome({
      tenant_id: "t1",
      agent_name: "sales-bot",
      kind: "revenue_usd",
      value: 500,
    });
    expect(ok).toBe(true);
  });

  it("outcomesSummary — returns parsed summary", async () => {
    const fixture = {
      tenant_id: "t1",
      range: "30d",
      summary: { by_kind: { revenue_usd: { count: 3, sum: 1500, avg: 500 } } },
    };
    const c = client(mockFetch(200, fixture));
    const result = await c.outcomesSummary({ tenant_id: "t1", range: "30d" });
    expect(result.summary.by_kind["revenue_usd"]!.sum).toBe(1500);
  });

  it("outcomesTimeseries — returns days array", async () => {
    const fixture = {
      tenant_id: "t1",
      range: "7d",
      days: [{ date: "2026-05-25", kind: "revenue_usd", count: 2, sum: 800 }],
    };
    const c = client(mockFetch(200, fixture));
    const result = await c.outcomesTimeseries({ tenant_id: "t1", range: "7d" });
    expect(result.days).toHaveLength(1);
    expect(result.days[0]!.sum).toBe(800);
  });

  it("listOutcomes — throws NotImplementedError", () => {
    const c = client(mockFetch(200, []));
    expect(() => c.listOutcomes()).toThrow(NotImplementedError);
  });
});

// ---------------------------------------------------------------------------
// Skills — happy-path tests
// ---------------------------------------------------------------------------

describe("Skills", () => {
  const packFixture: InstalledSkillPack = {
    id: "row-id-1",
    tenant_id: "t1",
    pack_slug: "rag-legal",
    version: "1.0.0",
    config: {},
    installed_at: "2026-05-25T00:00:00Z",
  };

  it("listInstalledSkills — returns array", async () => {
    const c = client(mockFetch(200, [packFixture]));
    const result = await c.listInstalledSkills("t1");
    expect(result[0]!.pack_slug).toBe("rag-legal");
  });

  it("listInstalledSkills — passes tenant query param", async () => {
    const { fetch: fn, lastUrl } = capturingFetch(200, []);
    const c = client(fn);
    await c.listInstalledSkills("my-tenant");
    expect(lastUrl()).toContain("tenant=my-tenant");
  });

  it("listSkillCatalog — extracts packs from catalog response", async () => {
    const catalog = {
      version: 1,
      packs: [{ slug: "rag-hr", name: "RAG HR", description: "...", version: "1.0.0", category: "hr" }],
    };
    const c = client(mockFetch(200, catalog));
    const packs = await c.listSkillCatalog();
    expect(packs).toHaveLength(1);
    expect(packs[0]!.slug).toBe("rag-hr");
  });

  it("installSkill — sends POST, returns installed pack", async () => {
    const fn = mockFetch(200, packFixture);
    const c = client(fn);
    const result = await c.installSkill({ tenant_id: "t1", pack_slug: "rag-legal" });
    expect(result.id).toBe("row-id-1");
    expect((fn as ReturnType<typeof vi.fn>).mock.calls[0]![1]!.method).toBe("POST");
  });

  it("uninstallSkill — returns deleted id", async () => {
    const c = client(mockFetch(200, { deleted: "row-id-1" }));
    const deleted = await c.uninstallSkill("row-id-1");
    expect(deleted).toBe("row-id-1");
  });
});

// ---------------------------------------------------------------------------
// Error mapping tests
// ---------------------------------------------------------------------------

describe("Error mapping", () => {
  it("401 → AuthError", async () => {
    const c = client(mockFetch(401, { error: "Unauthorized" }));
    await expect(c.listHotlPolicies({ tenant_id: "t1" })).rejects.toThrow(AuthError);
  });

  it("401 → is instance of HttpError and XiaoguaiError", async () => {
    const c = client(mockFetch(401, {}));
    const err = await c.listHotlPolicies({ tenant_id: "t1" }).catch((e) => e);
    expect(err).toBeInstanceOf(HttpError);
    expect(err).toBeInstanceOf(XiaoguaiError);
    expect(err.status).toBe(401);
  });

  it("403 → ForbiddenError", async () => {
    const c = client(mockFetch(403, { error: "Forbidden" }));
    await expect(c.listSkillCatalog()).rejects.toThrow(ForbiddenError);
  });

  it("404 → NotFoundError", async () => {
    const c = client(mockFetch(404, { error: "not found" }));
    await expect(c.deleteHotlPolicy("bad-id")).rejects.toThrow(NotFoundError);
  });

  it("409 → ConflictError", async () => {
    const c = client(mockFetch(409, { error: "pack already installed" }));
    await expect(
      c.installSkill({ tenant_id: "t1", pack_slug: "rag-hr" }),
    ).rejects.toThrow(ConflictError);
  });

  it("422 → ValidationError", async () => {
    const c = client(mockFetch(422, { error: "invalid request" }));
    await expect(
      c.createHotlPolicy({ tenant_id: "t1", scope: "llm", window_seconds: -1 }),
    ).rejects.toThrow(ValidationError);
  });

  it("400 → ValidationError", async () => {
    const c = client(mockFetch(400, { error: "bad request" }));
    await expect(
      c.recordOutcome({ tenant_id: "", agent_name: "", kind: "", value: -1 }),
    ).rejects.toThrow(ValidationError);
  });

  it("429 → RateLimitError with retryAfter", async () => {
    const c = client(mockFetch(429, { error: "rate limited" }, { "retry-after": "60" }));
    const err = await c.listHotlPolicies({ tenant_id: "t1" }).catch((e) => e);
    expect(err).toBeInstanceOf(RateLimitError);
    expect(err.retryAfter).toBe(60);
  });

  it("500 → ServerError", async () => {
    const c = client(mockFetch(500, { error: "internal server error" }));
    await expect(c.listHotlPolicies({ tenant_id: "t1" })).rejects.toThrow(ServerError);
  });

  it("503 → ServerError", async () => {
    const c = client(mockFetch(503, { error: "service unavailable" }));
    await expect(c.outcomesSummary({ tenant_id: "t1" })).rejects.toThrow(ServerError);
  });
});

// ---------------------------------------------------------------------------
// AbortSignal cancellation
// ---------------------------------------------------------------------------

describe("AbortSignal", () => {
  it("aborts in-flight request when signal fires", async () => {
    const controller = new AbortController();
    let fetchCalled = false;
    const slowFetch = vi.fn(async (_url: RequestInfo | URL, init?: RequestInit) => {
      fetchCalled = true;
      // Wait until the signal aborts.
      return new Promise<Response>((_resolve, reject) => {
        if (init?.signal?.aborted) {
          reject(new DOMException("aborted", "AbortError"));
          return;
        }
        const onAbort = () => reject(new DOMException("aborted", "AbortError"));
        init?.signal?.addEventListener("abort", onAbort, { once: true });
      });
    }) as unknown as typeof fetch;

    const c = new XiaoguaiClient({ baseUrl: BASE, token: "tok", fetch: slowFetch });
    controller.abort();

    await expect(
      c.listHotlPolicies({ tenant_id: "t1" }, controller.signal),
    ).rejects.toThrow();
    expect(fetchCalled).toBe(true);
  });

  it("pre-aborted signal propagates to fetch call", async () => {
    const controller = new AbortController();
    controller.abort();
    // Simulate a fetch that honours a pre-aborted signal by rejecting.
    const abortAwareFetch = vi.fn(async (_url: RequestInfo | URL, init?: RequestInit) => {
      if (init?.signal?.aborted) {
        throw new DOMException("signal aborted", "AbortError");
      }
      return new Response(JSON.stringify([]), {
        status: 200,
        headers: new Headers({ "Content-Type": "application/json" }),
      });
    }) as unknown as typeof fetch;

    const c = new XiaoguaiClient({ baseUrl: BASE, token: "tok", fetch: abortAwareFetch });
    await expect(
      c.listHotlPolicies({ tenant_id: "t1" }, controller.signal),
    ).rejects.toThrow();
  });
});

// ---------------------------------------------------------------------------
// Custom fetch injection
// ---------------------------------------------------------------------------

describe("Custom fetch", () => {
  it("uses the provided fetch function instead of global", async () => {
    let called = false;
    const customFetch = vi.fn(async () => {
      called = true;
      return new Response(JSON.stringify([]), {
        status: 200,
        headers: new Headers({ "Content-Type": "application/json" }),
      });
    }) as unknown as typeof fetch;

    const c = new XiaoguaiClient({ baseUrl: BASE, fetch: customFetch });
    await c.listSkillCatalog();
    expect(called).toBe(true);
  });

  it("sends Authorization header when token is provided", async () => {
    let capturedHeaders: HeadersInit | undefined;
    const fn = vi.fn(async (_url: RequestInfo | URL, init?: RequestInit) => {
      capturedHeaders = init?.headers;
      return new Response(JSON.stringify({ version: 1, packs: [] }), {
        status: 200,
        headers: new Headers({ "Content-Type": "application/json" }),
      });
    }) as unknown as typeof fetch;

    const c = new XiaoguaiClient({ baseUrl: BASE, token: "secret-token", fetch: fn });
    await c.listSkillCatalog();
    const headers = capturedHeaders as Record<string, string>;
    expect(headers["Authorization"]).toBe("Bearer secret-token");
  });

  it("omits Authorization header when no token", async () => {
    let capturedHeaders: HeadersInit | undefined;
    const fn = vi.fn(async (_url: RequestInfo | URL, init?: RequestInit) => {
      capturedHeaders = init?.headers;
      return new Response(JSON.stringify({ version: 1, packs: [] }), {
        status: 200,
        headers: new Headers({ "Content-Type": "application/json" }),
      });
    }) as unknown as typeof fetch;

    const c = new XiaoguaiClient({ baseUrl: BASE, fetch: fn });
    await c.listSkillCatalog();
    const headers = capturedHeaders as Record<string, string>;
    expect(headers["Authorization"]).toBeUndefined();
  });
});
