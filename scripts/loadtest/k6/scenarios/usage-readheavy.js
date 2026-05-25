/**
 * scenarios/usage-readheavy.js
 *
 * Read-heavy mix against the analytics / list endpoints:
 *   - GET /v1/usage          (60 % of iterations)
 *   - GET /v1/sessions       (40 % of iterations)
 *
 * Models a monitoring dashboard polling frequently alongside normal
 * session-list traffic.  40 VUs, 5 minutes.
 *
 * Thresholds (fail the test on breach):
 *   p95 response time < 500 ms
 *   error rate         < 1 %
 *
 * Required env vars:
 *   BASE_URL   — defaults to http://localhost:7600
 *   API_TOKEN  — bearer token (optional on dev stacks)
 */

import { sleep } from "k6";
import {
  checkResponse,
  fetchUsage,
  fetchSessions,
  usageLatency,
} from "../lib/common.js";

export const options = {
  scenarios: {
    usage_readheavy: {
      executor: "constant-vus",
      vus: 40,
      duration: "5m",
    },
  },
  thresholds: {
    http_req_duration: ["p(95)<500"],
    http_req_failed: ["rate<0.01"],
    usage_latency: ["p(95)<500"],
  },
};

export default function () {
  if (Math.random() < 0.6) {
    // GET /v1/usage — usage summary (today window by default)
    const res = fetchUsage("window=24h");
    checkResponse(res, "usage-readheavy GET /usage");
  } else {
    // GET /v1/sessions — session list
    const res = fetchSessions(20);
    checkResponse(res, "usage-readheavy GET /sessions");
  }

  // Short poll interval: simulate a dashboard refreshing every 1-2 s.
  sleep(0.5 + Math.random() * 1.5);
}
