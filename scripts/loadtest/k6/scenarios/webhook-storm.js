/**
 * scenarios/webhook-storm.js
 *
 * 100 VUs hammering the scheduler webhook routes for 3 minutes to validate
 * C15 rate-limiting behaviour.  The test expects the server to:
 *   - Accept requests within the allowed burst (200 OK / 202 Accepted)
 *   - Return 429 Too Many Requests for excess traffic — this is CORRECT
 *     behaviour and is NOT counted as an error in thresholds.
 *   - Never return 5xx (server should never crash under load)
 *
 * Thresholds (fail the test on breach):
 *   p95 response time < 500 ms
 *   http_req_failed    < 1 %   (only 4xx-that-aren't-429 and 5xx count)
 *
 * Required env vars:
 *   WEBHOOK_ROUTE_ID     — a pre-created webhook route ID
 *   WEBHOOK_ROUTE_TOKEN  — token for that route (optional on dev stacks)
 *   BASE_URL             — defaults to http://localhost:7600
 */

import { check, sleep } from "k6";
import http from "k6/http";
import {
  BASE_URL,
  checkResponse,
  triggerWebhook,
  webhookRateLimited,
} from "../lib/common.js";

export const options = {
  scenarios: {
    webhook_storm: {
      executor: "constant-vus",
      vus: 100,
      duration: "3m",
    },
  },
  thresholds: {
    http_req_duration: ["p(95)<500"],
    // Rate-limited (429) responses are acceptable; only genuine errors count.
    http_req_failed: ["rate<0.01"],
    // We expect the server to rate-limit; track the 429 rate separately.
    webhook_rate_limited: ["rate<0.80"],
  },
};

export default function () {
  const routeId = __ENV.WEBHOOK_ROUTE_ID;
  if (!routeId) {
    throw new Error(
      "WEBHOOK_ROUTE_ID env var is required.  " +
        "Create a webhook route first: POST /v1/scheduler/webhooks"
    );
  }

  const token = __ENV.WEBHOOK_ROUTE_TOKEN || "";
  const body = { event: "k6-storm", ts: Date.now() };

  const res = triggerWebhook(routeId, token, body);

  // 429 is the expected rate-limit response — treat as success for the
  // purpose of http_req_failed (k6 marks non-2xx as failed by default,
  // so we override by checking explicitly).
  check(res, {
    "webhook-storm: accepted or rate-limited": (r) =>
      [200, 201, 202, 429].includes(r.status),
    "webhook-storm: no server error": (r) => r.status < 500,
  });

  // No deliberate think-time: we want sustained pressure to trigger limiting.
  sleep(0.1);
}
