/**
 * tests/k6/wave3-hotl-check.js
 *
 * Wave-3 load test: POST /v1/hotl/check
 *
 * SLO target: p95 < 25 ms at 1,000 RPS sustained.
 * (perf-budget-wave3.md §2 — 1 policy lookup + 1 window SUM on indexed
 * `occurred_at` column; escalation fan-out is fire-and-forget.)
 *
 * Ramp profile:
 *   Stage 1: 0 → 1,000 VUs over 2 min  (warm-up, connection pool fill)
 *   Stage 2: 1,000 VUs for 5 min        (sustained throughput measurement)
 *   Stage 3: 1,000 → 0 VUs over 30 s   (graceful drain)
 *
 * Payload rotates across 20 tenant UUIDs × 4 scopes to exercise the
 * policy-lookup index realistically.
 *
 * Required env vars:
 *   BASE_URL   — defaults to http://localhost:7600
 *   API_TOKEN  — Bearer token (optional on dev stacks that skip auth)
 *
 * Run:
 *   k6 run tests/k6/wave3-hotl-check.js
 *   BASE_URL=https://staging.example.com API_TOKEN=<tok> k6 run tests/k6/wave3-hotl-check.js
 */

import { check, sleep } from "k6";
import http from "k6/http";
import { Counter, Rate, Trend } from "k6/metrics";

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

const BASE_URL = __ENV.BASE_URL || "http://localhost:7600";

// ---------------------------------------------------------------------------
// Custom metrics
// ---------------------------------------------------------------------------

/** End-to-end duration for /v1/hotl/check specifically. */
const hotlCheckLatency = new Trend("hotl_check_latency", true);

/** Count of hotl/check responses that returned a Deny or Escalate verdict. */
const hotlDenied = new Counter("hotl_denied");

/** Requests that breached the 25 ms p95 SLO individually. */
const sloBreaches = new Counter("hotl_check_slo_breaches");

/** Rate of non-2xx responses (error rate). */
const errorRate = new Rate("hotl_check_error_rate");

// ---------------------------------------------------------------------------
// Test data
// ---------------------------------------------------------------------------

// 20 rotating tenant UUIDs — avoids single-tenant cache saturation.
const TENANT_IDS = [
  "00000000-0000-0000-0000-000000000001",
  "00000000-0000-0000-0000-000000000002",
  "00000000-0000-0000-0000-000000000003",
  "00000000-0000-0000-0000-000000000004",
  "00000000-0000-0000-0000-000000000005",
  "00000000-0000-0000-0000-000000000006",
  "00000000-0000-0000-0000-000000000007",
  "00000000-0000-0000-0000-000000000008",
  "00000000-0000-0000-0000-000000000009",
  "00000000-0000-0000-0000-000000000010",
  "00000000-0000-0000-0000-000000000011",
  "00000000-0000-0000-0000-000000000012",
  "00000000-0000-0000-0000-000000000013",
  "00000000-0000-0000-0000-000000000014",
  "00000000-0000-0000-0000-000000000015",
  "00000000-0000-0000-0000-000000000016",
  "00000000-0000-0000-0000-000000000017",
  "00000000-0000-0000-0000-000000000018",
  "00000000-0000-0000-0000-000000000019",
  "00000000-0000-0000-0000-000000000020",
];

// Scopes mirror the wired action sites from enforcer.rs comments.
const SCOPES = ["llm_call", "email_send", "webhook_invoke", "tool_call"];

// Amounts: invocation counting (1.0) or small USD cost floats.
const AMOUNTS = [1.0, 0.002, 0.005, 0.01, 1.0];

function randomTenant() {
  return TENANT_IDS[Math.floor(Math.random() * TENANT_IDS.length)];
}

function randomScope() {
  return SCOPES[Math.floor(Math.random() * SCOPES.length)];
}

function randomAmount() {
  return AMOUNTS[Math.floor(Math.random() * AMOUNTS.length)];
}

// ---------------------------------------------------------------------------
// k6 options
// ---------------------------------------------------------------------------

export const options = {
  scenarios: {
    hotl_check_ramp: {
      executor: "ramping-vus",
      startVUs: 0,
      stages: [
        { duration: "2m", target: 1000 }, // ramp: 0 → 1000 VUs
        { duration: "5m", target: 1000 }, // sustain at 1000 VUs
        { duration: "30s", target: 0 },   // drain
      ],
      gracefulRampDown: "30s",
    },
  },
  thresholds: {
    // Primary SLO: p95 < 25 ms over the sustained window.
    // Tag-scoped so it only covers hotl-check requests (not any overhead).
    "http_req_duration{name:hotl-check}": ["p(95)<25"],
    // Custom trend mirrors the tagged duration for dashboard clarity.
    hotl_check_latency: ["p(95)<25"],
    // Error rate must stay below 0.5% (perf-budget §4.2 alarm condition).
    hotl_check_error_rate: ["rate<0.005"],
    // Generic safety net.
    http_req_failed: ["rate<0.01"],
  },
};

// ---------------------------------------------------------------------------
// Auth headers
// ---------------------------------------------------------------------------

function authHeaders() {
  const h = { "Content-Type": "application/json" };
  if (__ENV.API_TOKEN) {
    h["Authorization"] = `Bearer ${__ENV.API_TOKEN}`;
  }
  return h;
}

// ---------------------------------------------------------------------------
// Default function (VU loop)
// ---------------------------------------------------------------------------

export default function () {
  const payload = JSON.stringify({
    tenant_id: randomTenant(),
    scope: randomScope(),
    amount: randomAmount(),
  });

  const res = http.post(`${BASE_URL}/v1/hotl/check`, payload, {
    headers: authHeaders(),
    tags: { name: "hotl-check" },
  });

  // Record custom metrics.
  hotlCheckLatency.add(res.timings.duration);
  if (res.timings.duration > 25) {
    sloBreaches.add(1);
  }

  const ok = check(res, {
    "hotl-check: status 2xx or 402/429": (r) =>
      [200, 201, 402, 429].includes(r.status),
    "hotl-check: body non-empty": (r) => r.body && r.body.length > 0,
  });

  errorRate.add(!ok ? 1 : 0);

  // Track Deny / Escalate verdicts from the response body.
  if (res.status === 200 || res.status === 201) {
    try {
      const body = JSON.parse(res.body);
      if (body && (body.verdict === "Deny" || body.verdict === "Escalate")) {
        hotlDenied.add(1);
      }
    } catch (_) {
      // non-JSON body — not an error condition, just skip verdict tracking.
    }
  }

  // No sleep: hotl/check is a tight inner loop; VUs model concurrent agents,
  // not human users. The ramp profile controls throughput.
}
