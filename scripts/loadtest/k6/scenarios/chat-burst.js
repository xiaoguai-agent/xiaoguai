/**
 * scenarios/chat-burst.js
 *
 * Sustained chat load: 50 VUs posting to POST /v1/sessions/:id/messages
 * for 5 minutes.  Models the steady-state worst case of a busy team
 * all chatting simultaneously.
 *
 * Thresholds (fail the test on breach):
 *   p95 response time < 500 ms
 *   error rate         < 1 %
 *
 * Required env vars:
 *   SESSION_ID   — a pre-created session UUID
 *   BASE_URL     — defaults to http://localhost:7600
 *   API_TOKEN    — bearer token (optional on dev stacks)
 */

import { sleep } from "k6";
import {
  authHeaders,
  BASE_URL,
  checkResponse,
  chatLatency,
  sendMessage,
  randomQuestion,
} from "../lib/common.js";

export const options = {
  scenarios: {
    chat_burst: {
      executor: "constant-vus",
      vus: 50,
      duration: "5m",
    },
  },
  thresholds: {
    // p95 end-to-end latency must stay below 500 ms
    http_req_duration: ["p(95)<500"],
    // fewer than 1 % of all requests may fail
    http_req_failed: ["rate<0.01"],
    // also gate on the custom chat-specific trend
    chat_latency: ["p(95)<500"],
  },
};

export default function () {
  const sessionId = __ENV.SESSION_ID;
  if (!sessionId) {
    throw new Error(
      "SESSION_ID env var is required.  " +
        "Create a session first: POST /v1/sessions"
    );
  }

  const res = sendMessage(sessionId, randomQuestion());
  checkResponse(res, "chat-burst POST /messages", [200, 201]);

  // Think-time: simulate a user reading the reply before typing again.
  // 1–3 s keeps throughput realistic without hammering the LLM mock.
  sleep(1 + Math.random() * 2);
}
