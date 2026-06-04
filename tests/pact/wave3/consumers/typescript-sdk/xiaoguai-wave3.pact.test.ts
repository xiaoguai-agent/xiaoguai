/**
 * Pact consumer contract tests — TypeScript SDK vs. xiaoguai wave-3 API.
 *
 * Consumer: typescript-sdk
 * Provider: xiaoguai
 * Pact spec: v3
 *
 * Covers 12 interactions across:
 *   - HotL CRUD (list, create, get, update, delete) + check
 *   - Outcomes (record, summary, timeseries)
 *   - Skills (list installed, install, uninstall)
 */

import { PactV3, MatchersV3 } from "@pact-foundation/pact";
import * as path from "path";

const { like, eachLike, string, uuid, integer, decimal, datetime, regex } =
  MatchersV3;

const PACT_DIR = path.resolve(
  __dirname,
  "../../pacts"
);

const provider = new PactV3({
  consumer: "typescript-sdk",
  provider: "xiaoguai",
  dir: PACT_DIR,
  spec: 3,
  logLevel: "warn",
});

// ────────────────────────────────────────────────────────────────────────────
// Constants shared across interactions
// ────────────────────────────────────────────────────────────────────────────

const POLICY_UUID = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
const BEARER = "Bearer test-token";

const POLICY_BODY = {
  id: uuid(),
  scope: string("llm_call"),
  window_seconds: integer(3600),
  max_count: integer(100),
  max_usd: decimal(5.0),
  escalate_to: string("ops@example.com"),
};

// ────────────────────────────────────────────────────────────────────────────
// Interaction 1: List HotL policies — 200
// ────────────────────────────────────────────────────────────────────────────

describe("typescript-sdk → xiaoguai wave-3", () => {
  describe("HotL policies", () => {
    it("GET /v1/hotl/policies returns 200 with policy array", () => {
      return provider
        .addInteraction({
          states: [{ description: "tenant has one HotL policy" }],
          uponReceiving:
            "a GET /v1/hotl/policies request for tenant 11111111",
          withRequest: {
            method: "GET",
            path: "/v1/hotl/policies",
            headers: { Authorization: BEARER },
          },
          willRespondWith: {
            status: 200,
            headers: { "Content-Type": "application/json" },
            body: eachLike(POLICY_BODY),
          },
        })
        .executeTest(async (mockServer) => {
          const res = await fetch(
            `${mockServer.url}/v1/hotl/policies`,
            { headers: { Authorization: BEARER } }
          );
          expect(res.status).toBe(200);
          const body = (await res.json()) as any;
          expect(Array.isArray(body)).toBe(true);
          expect(body.length).toBeGreaterThan(0);
        });
    });

    // ────────────────────────────────────────────────────────────────────────
    // Interaction 2: Create HotL policy — 201
    // ────────────────────────────────────────────────────────────────────────

    it("POST /v1/hotl/policies returns 201 with created policy", () => {
      return provider
        .addInteraction({
          states: [{ description: "HotL policy store is available" }],
          uponReceiving: "a POST /v1/hotl/policies request",
          withRequest: {
            method: "POST",
            path: "/v1/hotl/policies",
            headers: {
              Authorization: BEARER,
              "Content-Type": "application/json",
            },
            body: {
              scope: "llm_call",
              window_seconds: 3600,
              max_count: 100,
              max_usd: 5.0,
              escalate_to: "ops@example.com",
            },
          },
          willRespondWith: {
            status: 201,
            headers: { "Content-Type": "application/json" },
            body: POLICY_BODY,
          },
        })
        .executeTest(async (mockServer) => {
          const res = await fetch(`${mockServer.url}/v1/hotl/policies`, {
            method: "POST",
            headers: {
              Authorization: BEARER,
              "Content-Type": "application/json",
            },
            body: JSON.stringify({
              scope: "llm_call",
              window_seconds: 3600,
              max_count: 100,
              max_usd: 5.0,
              escalate_to: "ops@example.com",
            }),
          });
          expect(res.status).toBe(201);
          const body = (await res.json()) as any;
          expect(body).toHaveProperty("id");
          expect(body.scope).toBe("llm_call");
        });
    });

    // ────────────────────────────────────────────────────────────────────────
    // Interaction 3: Get single HotL policy — 200
    // ────────────────────────────────────────────────────────────────────────

    it("GET /v1/hotl/policies/:id returns 200 with policy", () => {
      return provider
        .addInteraction({
          states: [
            {
              description: "HotL policy exists",
              parameters: { id: POLICY_UUID },
            },
          ],
          uponReceiving: `a GET /v1/hotl/policies/${POLICY_UUID} request`,
          withRequest: {
            method: "GET",
            path: `/v1/hotl/policies/${POLICY_UUID}`,
            headers: { Authorization: BEARER },
          },
          willRespondWith: {
            status: 200,
            headers: { "Content-Type": "application/json" },
            body: POLICY_BODY,
          },
        })
        .executeTest(async (mockServer) => {
          const res = await fetch(
            `${mockServer.url}/v1/hotl/policies/${POLICY_UUID}`,
            { headers: { Authorization: BEARER } }
          );
          expect(res.status).toBe(200);
          const body = (await res.json()) as any;
          expect(body).toHaveProperty("id");
        });
    });

    // ────────────────────────────────────────────────────────────────────────
    // Interaction 4: Update HotL policy — 200
    // ────────────────────────────────────────────────────────────────────────

    it("PUT /v1/hotl/policies/:id returns 200 with updated policy", () => {
      return provider
        .addInteraction({
          states: [
            {
              description: "HotL policy exists",
              parameters: { id: POLICY_UUID },
            },
          ],
          uponReceiving: `a PUT /v1/hotl/policies/${POLICY_UUID} request`,
          withRequest: {
            method: "PUT",
            path: `/v1/hotl/policies/${POLICY_UUID}`,
            headers: {
              Authorization: BEARER,
              "Content-Type": "application/json",
            },
            body: {
              scope: "llm_call",
              window_seconds: 7200,
              max_count: 200,
              max_usd: null,
              escalate_to: null,
            },
          },
          willRespondWith: {
            status: 200,
            headers: { "Content-Type": "application/json" },
            body: {
              ...POLICY_BODY,
              window_seconds: integer(7200),
              max_count: integer(200),
            },
          },
        })
        .executeTest(async (mockServer) => {
          const res = await fetch(
            `${mockServer.url}/v1/hotl/policies/${POLICY_UUID}`,
            {
              method: "PUT",
              headers: {
                Authorization: BEARER,
                "Content-Type": "application/json",
              },
              body: JSON.stringify({
                scope: "llm_call",
                window_seconds: 7200,
                max_count: 200,
                max_usd: null,
                escalate_to: null,
              }),
            }
          );
          expect(res.status).toBe(200);
        });
    });

    // ────────────────────────────────────────────────────────────────────────
    // Interaction 5: Delete HotL policy — 204
    // ────────────────────────────────────────────────────────────────────────

    it("DELETE /v1/hotl/policies/:id returns 204", () => {
      return provider
        .addInteraction({
          states: [
            {
              description: "HotL policy exists",
              parameters: { id: POLICY_UUID },
            },
          ],
          uponReceiving: `a DELETE /v1/hotl/policies/${POLICY_UUID} request`,
          withRequest: {
            method: "DELETE",
            path: `/v1/hotl/policies/${POLICY_UUID}`,
            headers: { Authorization: BEARER },
          },
          willRespondWith: {
            status: 204,
          },
        })
        .executeTest(async (mockServer) => {
          const res = await fetch(
            `${mockServer.url}/v1/hotl/policies/${POLICY_UUID}`,
            {
              method: "DELETE",
              headers: { Authorization: BEARER },
            }
          );
          expect(res.status).toBe(204);
        });
    });

    // ────────────────────────────────────────────────────────────────────────
    // Interaction 6: HotL check — allow verdict
    // ────────────────────────────────────────────────────────────────────────

    it("POST /v1/hotl/check returns allow verdict", () => {
      return provider
        .addInteraction({
          states: [
            {
              description: "tenant HotL policy exists and budget is within limits",
            },
          ],
          uponReceiving: "a POST /v1/hotl/check request within budget",
          withRequest: {
            method: "POST",
            path: "/v1/hotl/check",
            headers: {
              Authorization: BEARER,
              "Content-Type": "application/json",
            },
            body: {
              scope: "llm_call",
              amount: 0.0025,
            },
          },
          willRespondWith: {
            status: 200,
            headers: { "Content-Type": "application/json" },
            body: {
              verdict: string("allow"),
              reason: null,
            },
          },
        })
        .executeTest(async (mockServer) => {
          const res = await fetch(`${mockServer.url}/v1/hotl/check`, {
            method: "POST",
            headers: {
              Authorization: BEARER,
              "Content-Type": "application/json",
            },
            body: JSON.stringify({
              scope: "llm_call",
              amount: 0.0025,
            }),
          });
          expect(res.status).toBe(200);
          const body = (await res.json()) as any;
          expect(body.verdict).toBe("allow");
        });
    });
  });

  // ──────────────────────────────────────────────────────────────────────────
  // Outcomes
  // ──────────────────────────────────────────────────────────────────────────

  describe("Outcomes", () => {
    // ────────────────────────────────────────────────────────────────────────
    // Interaction 7: Record outcome — 201
    // ────────────────────────────────────────────────────────────────────────

    it("POST /v1/outcomes returns 201 with ok: true", () => {
      return provider
        .addInteraction({
          states: [{ description: "outcome writer is available" }],
          uponReceiving: "a POST /v1/outcomes request",
          withRequest: {
            method: "POST",
            path: "/v1/outcomes",
            headers: {
              Authorization: BEARER,
              "Content-Type": "application/json",
            },
            body: {
              session_id: "sess_abc123",
              agent_name: "sales-bot",
              kind: "revenue_usd",
              value: 1250.0,
              unit: "usd",
              description: "Closed deal D-4471",
              metadata: { deal_id: "D-4471" },
            },
          },
          willRespondWith: {
            status: 201,
            headers: { "Content-Type": "application/json" },
            body: { ok: true },
          },
        })
        .executeTest(async (mockServer) => {
          const res = await fetch(`${mockServer.url}/v1/outcomes`, {
            method: "POST",
            headers: {
              Authorization: BEARER,
              "Content-Type": "application/json",
            },
            body: JSON.stringify({
              session_id: "sess_abc123",
              agent_name: "sales-bot",
              kind: "revenue_usd",
              value: 1250.0,
              unit: "usd",
              description: "Closed deal D-4471",
              metadata: { deal_id: "D-4471" },
            }),
          });
          expect(res.status).toBe(201);
          const body = (await res.json()) as any;
          expect(body.ok).toBe(true);
        });
    });

    // ────────────────────────────────────────────────────────────────────────
    // Interaction 8: Outcomes summary — 200
    // ────────────────────────────────────────────────────────────────────────

    it("GET /v1/outcomes/summary returns 200 with aggregated summary", () => {
      return provider
        .addInteraction({
          states: [{ description: "tenant has recorded outcomes" }],
          uponReceiving: "a GET /v1/outcomes/summary request for 7d",
          withRequest: {
            method: "GET",
            path: "/v1/outcomes/summary",
            query: { range: "7d" },
            headers: { Authorization: BEARER },
          },
          willRespondWith: {
            status: 200,
            headers: { "Content-Type": "application/json" },
            body: {
              range: string("7d"),
              summary: {
                by_kind: like({
                  revenue_usd: {
                    sum: decimal(42000.0),
                    count: integer(18),
                    avg: decimal(2333.33),
                  },
                }),
              },
            },
          },
        })
        .executeTest(async (mockServer) => {
          const res = await fetch(
            `${mockServer.url}/v1/outcomes/summary?range=7d`,
            { headers: { Authorization: BEARER } }
          );
          expect(res.status).toBe(200);
          const body = (await res.json()) as any;
          expect(body).toHaveProperty("summary");
          expect(body.summary).toHaveProperty("by_kind");
        });
    });

    // ────────────────────────────────────────────────────────────────────────
    // Interaction 9: Outcomes timeseries — 200
    // ────────────────────────────────────────────────────────────────────────

    it("GET /v1/outcomes/timeseries returns 200 with daily buckets", () => {
      return provider
        .addInteraction({
          states: [{ description: "tenant has recorded outcomes" }],
          uponReceiving: "a GET /v1/outcomes/timeseries request for 7d",
          withRequest: {
            method: "GET",
            path: "/v1/outcomes/timeseries",
            query: { range: "7d" },
            headers: { Authorization: BEARER },
          },
          willRespondWith: {
            status: 200,
            headers: { "Content-Type": "application/json" },
            body: {
              range: string("7d"),
              days: eachLike({
                date: string("2026-05-20"),
                kind: string("revenue_usd"),
                sum: decimal(5000.0),
                count: integer(2),
              }),
            },
          },
        })
        .executeTest(async (mockServer) => {
          const res = await fetch(
            `${mockServer.url}/v1/outcomes/timeseries?range=7d`,
            { headers: { Authorization: BEARER } }
          );
          expect(res.status).toBe(200);
          const body = (await res.json()) as any;
          expect(Array.isArray(body.days)).toBe(true);
        });
    });
  });

  // ──────────────────────────────────────────────────────────────────────────
  // Skills
  // ──────────────────────────────────────────────────────────────────────────

  describe("Skills", () => {
    const INSTALL_UUID = "cccccccc-cccc-cccc-cccc-cccccccccccc";

    const INSTALLED_PACK_BODY = {
      id: uuid(),
      pack_slug: string("pr-review"),
      version: string("1.0.0"),
      config: like({}),
      installed_at: string("2026-05-25T12:34:56Z"),
    };

    // ────────────────────────────────────────────────────────────────────────
    // Interaction 10: List installed skills — 200
    // ────────────────────────────────────────────────────────────────────────

    it("GET /v1/skills/installed returns 200 with installed packs", () => {
      return provider
        .addInteraction({
          states: [{ description: "tenant has installed skill packs" }],
          uponReceiving: "a GET /v1/skills/installed request",
          withRequest: {
            method: "GET",
            path: "/v1/skills/installed",
            headers: { Authorization: BEARER },
          },
          willRespondWith: {
            status: 200,
            headers: { "Content-Type": "application/json" },
            body: eachLike(INSTALLED_PACK_BODY),
          },
        })
        .executeTest(async (mockServer) => {
          const res = await fetch(
            `${mockServer.url}/v1/skills/installed`,
            { headers: { Authorization: BEARER } }
          );
          expect(res.status).toBe(200);
          const body = (await res.json()) as any;
          expect(Array.isArray(body)).toBe(true);
        });
    });

    // ────────────────────────────────────────────────────────────────────────
    // Interaction 11: Install skill pack — 201
    // ────────────────────────────────────────────────────────────────────────

    it("POST /v1/skills/install returns 201 with installed pack row", () => {
      return provider
        .addInteraction({
          states: [
            {
              description: "skill pack pr-review exists in catalog",
            },
          ],
          uponReceiving: "a POST /v1/skills/install request",
          withRequest: {
            method: "POST",
            path: "/v1/skills/install",
            headers: {
              Authorization: BEARER,
              "Content-Type": "application/json",
            },
            body: {
              pack_slug: "pr-review",
              config: {},
            },
          },
          willRespondWith: {
            status: 201,
            headers: { "Content-Type": "application/json" },
            body: INSTALLED_PACK_BODY,
          },
        })
        .executeTest(async (mockServer) => {
          const res = await fetch(`${mockServer.url}/v1/skills/install`, {
            method: "POST",
            headers: {
              Authorization: BEARER,
              "Content-Type": "application/json",
            },
            body: JSON.stringify({
              pack_slug: "pr-review",
              config: {},
            }),
          });
          expect(res.status).toBe(201);
          const body = (await res.json()) as any;
          expect(body).toHaveProperty("id");
          expect(body.pack_slug).toBe("pr-review");
        });
    });

    // ────────────────────────────────────────────────────────────────────────
    // Interaction 12: Uninstall skill pack — 204
    // ────────────────────────────────────────────────────────────────────────

    it("DELETE /v1/skills/install/:id returns 204", () => {
      return provider
        .addInteraction({
          states: [
            {
              description: "skill pack installation exists",
              parameters: { id: INSTALL_UUID },
            },
          ],
          uponReceiving: `a DELETE /v1/skills/install/${INSTALL_UUID} request`,
          withRequest: {
            method: "DELETE",
            path: `/v1/skills/install/${INSTALL_UUID}`,
            headers: { Authorization: BEARER },
          },
          willRespondWith: {
            status: 204,
          },
        })
        .executeTest(async (mockServer) => {
          const res = await fetch(
            `${mockServer.url}/v1/skills/install/${INSTALL_UUID}`,
            {
              method: "DELETE",
              headers: { Authorization: BEARER },
            }
          );
          expect(res.status).toBe(204);
        });
    });
  });
});
