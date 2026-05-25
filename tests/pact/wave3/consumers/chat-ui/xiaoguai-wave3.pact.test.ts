/**
 * Pact consumer contract tests — chat-ui vs. xiaoguai wave-3 API.
 *
 * Consumer: chat-ui
 * Provider: xiaoguai
 * Pact spec: v3
 *
 * This consumer covers the 3 endpoints the chat-ui frontend calls directly:
 *
 *   1. GET  /v1/outcomes/summary?session_id=  — per-session ROI card
 *   2. POST /v1/hotl/check                    — live budget gate in the message composer
 *   3. GET  /v1/tenants/:id/config            — tenant config including ai_disclosure_banner
 *
 * IMPORTANT — CONTRACT GAP SURFACED:
 *   Interaction 3 (`GET /v1/tenants/:id/config`) is expected by chat-ui's
 *   `AiDisclosureBanner` component but the endpoint does NOT exist in the
 *   current xiaoguai-api router (see `crates/xiaoguai-api/src/routes/mod.rs`).
 *   The mock will define the interaction; the provider-verification step will
 *   FAIL for this interaction until the endpoint is implemented.
 *
 *   See: https://github.com/xiaoguai/xiaoguai/issues/TODO
 *   TODO: implement GET /v1/tenants/:id/config exposing ai_disclosure_banner
 *         (tracked in wave-4 backlog).
 */

import { PactV3, MatchersV3 } from "@pact-foundation/pact";
import * as path from "path";

const { like, string, boolean, uuid } = MatchersV3;

const PACT_DIR = path.resolve(
  __dirname,
  "../../pacts"
);

const provider = new PactV3({
  consumer: "chat-ui",
  provider: "xiaoguai",
  dir: PACT_DIR,
  spec: 3,
  logLevel: "warn",
});

const TENANT_UUID = "11111111-1111-1111-1111-111111111111";
const SESSION_ID = "sess_abc123";
const BEARER = "Bearer test-token";

describe("chat-ui → xiaoguai wave-3", () => {
  // ──────────────────────────────────────────────────────────────────────────
  // Interaction 1: GET /v1/outcomes/summary?session_id= — per-session ROI card
  // ──────────────────────────────────────────────────────────────────────────

  it("GET /v1/outcomes/summary with session_id returns 200 with summary scoped to session", () => {
    return provider
      .addInteraction({
        states: [
          {
            description: "tenant has recorded outcomes for session sess_abc123",
          },
        ],
        uponReceiving:
          "a GET /v1/outcomes/summary request with session_id filter",
        withRequest: {
          method: "GET",
          path: "/v1/outcomes/summary",
          query: {
            tenant_id: "tenant_acme",
            session_id: SESSION_ID,
            range: "30d",
          },
          headers: { Authorization: BEARER },
        },
        willRespondWith: {
          status: 200,
          headers: { "Content-Type": "application/json" },
          body: {
            tenant_id: string("tenant_acme"),
            range: string("30d"),
            summary: like({
              by_kind: like({
                revenue_usd: like({
                  sum: like(1250.0),
                  count: like(1),
                  avg: like(1250.0),
                }),
              }),
            }),
          },
        },
      })
      .executeTest(async (mockServer) => {
        const res = await fetch(
          `${mockServer.url}/v1/outcomes/summary?tenant_id=tenant_acme&session_id=${SESSION_ID}&range=30d`,
          { headers: { Authorization: BEARER } }
        );
        expect(res.status).toBe(200);
        const body = await res.json();
        expect(body).toHaveProperty("summary");
        expect(body.summary).toHaveProperty("by_kind");
      });
  });

  // ──────────────────────────────────────────────────────────────────────────
  // Interaction 2: POST /v1/hotl/check — live budget gate in message composer
  // ──────────────────────────────────────────────────────────────────────────

  it("POST /v1/hotl/check returns allow verdict when budget is OK", () => {
    return provider
      .addInteraction({
        states: [
          {
            description:
              "tenant HotL policy exists and budget is within limits",
          },
        ],
        uponReceiving:
          "a POST /v1/hotl/check from chat-ui before sending a message",
        withRequest: {
          method: "POST",
          path: "/v1/hotl/check",
          headers: {
            Authorization: BEARER,
            "Content-Type": "application/json",
          },
          body: {
            tenant_id: TENANT_UUID,
            scope: "llm_call",
            amount: 1.0,
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
            tenant_id: TENANT_UUID,
            scope: "llm_call",
            amount: 1.0,
          }),
        });
        expect(res.status).toBe(200);
        const body = await res.json();
        // chat-ui blocks send if verdict is not "allow"
        expect(body.verdict).toBe("allow");
      });
  });

  // ──────────────────────────────────────────────────────────────────────────
  // Interaction 3: GET /v1/tenants/:id/config — ai_disclosure_banner
  //
  // CONTRACT GAP: This endpoint does NOT yet exist on the provider.
  // chat-ui's AiDisclosureBanner component calls it on mount to determine
  // whether to render the EU AI Act / custom disclosure banner.
  //
  // Expected response shape (from chat-ui component code):
  //   { ai_disclosure_banner: { enabled: bool, text: string | null } }
  //
  // Provider status: NOT IMPLEMENTED — provider verification will return
  //   404 or 500 for this interaction until the route is added.
  //
  // Tracking: implement GET /v1/tenants/:id/config in wave-4.
  //   Candidate handler location: crates/xiaoguai-api/src/routes/admin.rs
  //   or a new crates/xiaoguai-api/src/routes/tenants.rs module.
  // ──────────────────────────────────────────────────────────────────────────

  it("GET /v1/tenants/:id/config returns 200 with ai_disclosure_banner [PROVIDER GAP]", () => {
    return provider
      .addInteraction({
        states: [
          {
            description: "tenant exists with ai_disclosure_banner configured",
            params: { tenant_id: TENANT_UUID },
          },
        ],
        uponReceiving: `a GET /v1/tenants/${TENANT_UUID}/config request for ai_disclosure_banner`,
        withRequest: {
          method: "GET",
          path: `/v1/tenants/${TENANT_UUID}/config`,
          headers: { Authorization: BEARER },
        },
        willRespondWith: {
          status: 200,
          headers: { "Content-Type": "application/json" },
          // Minimum shape chat-ui needs to render the banner component:
          body: {
            tenant_id: uuid(),
            ai_disclosure_banner: like({
              enabled: boolean(true),
              // text is nullable — null means use the default platform copy
              text: like(
                "This assistant is powered by AI. Responses may not be accurate."
              ),
            }),
          },
        },
      })
      .executeTest(async (mockServer) => {
        const res = await fetch(
          `${mockServer.url}/v1/tenants/${TENANT_UUID}/config`,
          { headers: { Authorization: BEARER } }
        );
        // When the provider is not implemented, this will be 404/500.
        // The test is written for the EXPECTED contract so provider
        // verification surfaces the gap automatically.
        expect(res.status).toBe(200);
        const body = await res.json();
        expect(body).toHaveProperty("ai_disclosure_banner");
        expect(typeof body.ai_disclosure_banner.enabled).toBe("boolean");
      });
  });
});
