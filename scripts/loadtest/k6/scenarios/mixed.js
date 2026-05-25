/**
 * scenarios/mixed.js
 *
 * Realistic traffic mix over 5 minutes at 60 VUs:
 *   70 % reads  — GET /v1/usage + GET /v1/sessions
 *   20 % chat   — POST /v1/sessions/:id/messages
 *   10 % admin  — GET /v1/scheduler/webhooks  (list, read-only)
 *
 * Designed to reflect actual production traffic distribution from
 * typical team deployments: most users are reading; a fraction is
 * actively chatting; a small slice is the admin pane.
 *
 * Thresholds (fail the test on breach):
 *   p95 response time < 500 ms
 *   error rate         < 1 %
 *
 * Required env vars:
 *   SESSION_ID   — a pre-created session UUID (needed for chat share)
 *   BASE_URL     — defaults to http://localhost:7600
 *   API_TOKEN    — bearer token (optional on dev stacks)
 */

import { sleep } from "k6";
import http from "k6/http";
import {
  authHeaders,
  BASE_URL,
  checkResponse,
  fetchSessions,
  fetchUsage,
  sendMessage,
  randomQuestion,
} from "../lib/common.js";

export const options = {
  scenarios: {
    mixed_realistic: {
      executor: "constant-vus",
      vus: 60,
      duration: "5m",
    },
  },
  thresholds: {
    http_req_duration: ["p(95)<500"],
    http_req_failed: ["rate<0.01"],
    // Also gate the chat-specific trend so chat slowdowns surface clearly.
    chat_latency: ["p(95)<500"],
  },
};

export default function () {
  const roll = Math.random();

  if (roll < 0.70) {
    // ---- read tier (70 %) ----
    if (Math.random() < 0.5) {
      const res = fetchUsage("window=24h");
      checkResponse(res, "mixed GET /usage");
    } else {
      const res = fetchSessions(20);
      checkResponse(res, "mixed GET /sessions");
    }
    sleep(0.5 + Math.random() * 1.0);

  } else if (roll < 0.90) {
    // ---- chat tier (20 %) ----
    const sessionId = __ENV.SESSION_ID;
    if (!sessionId) {
      // Degrade gracefully in CI where SESSION_ID is not set: skip chat turn.
      sleep(1);
      return;
    }
    const res = sendMessage(sessionId, randomQuestion());
    checkResponse(res, "mixed POST /messages", [200, 201]);
    sleep(1 + Math.random() * 2);

  } else {
    // ---- admin tier (10 %) ----
    const res = http.get(
      `${BASE_URL}/v1/scheduler/webhooks`,
      { headers: authHeaders() }
    );
    checkResponse(res, "mixed GET /scheduler/webhooks");
    sleep(0.5 + Math.random() * 1.0);
  }
}
