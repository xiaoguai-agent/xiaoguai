/**
 * Pact consumer contract tests — chat-ui vs. xiaoguai wave-3 API.
 *
 * Consumer: chat-ui
 * Provider: xiaoguai
 * Pact spec: v3
 *
 * This consumer covers the endpoints the chat-ui frontend calls directly:
 *
 *   1. GET  /v1/outcomes/summary?session_id=  — per-session ROI card
 *   2. POST /v1/hotl/check                    — live budget gate in the message composer
 */

import { PactV3, MatchersV3 } from "@pact-foundation/pact";
import * as path from "path";

const { like, string } = MatchersV3;

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
            session_id: SESSION_ID,
            range: "30d",
          },
          headers: { Authorization: BEARER },
        },
        willRespondWith: {
          status: 200,
          headers: { "Content-Type": "application/json" },
          body: {
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
          `${mockServer.url}/v1/outcomes/summary?session_id=${SESSION_ID}&range=30d`,
          { headers: { Authorization: BEARER } }
        );
        expect(res.status).toBe(200);
        const body = (await res.json()) as any;
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
            scope: "llm_call",
            amount: 1.0,
          }),
        });
        expect(res.status).toBe(200);
        const body = (await res.json()) as any;
        // chat-ui blocks send if verdict is not "allow"
        expect(body.verdict).toBe("allow");
      });
  });
});
