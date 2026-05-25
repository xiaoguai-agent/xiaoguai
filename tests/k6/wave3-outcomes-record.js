/**
 * tests/k6/wave3-outcomes-record.js
 *
 * Wave-3 load test: POST /v1/outcomes
 *
 * SLO target: p95 < 50 ms at 500 RPS sustained.
 * (perf-budget-wave3.md §2 — single INSERT into `agent_outcomes`; no
 * aggregation.  Daily-bucket design means writes are UPSERTs, not appends.)
 *
 * Uses `constant-arrival-rate` executor to achieve a stable 500 RPS
 * regardless of VU think-time, matching the throughput floor test methodology.
 *
 * Payload variation:
 *   - 6 outcome kinds (revenue_usd, cost_saved_usd, hours_saved,
 *     deals_closed, tickets_resolved, custom)
 *   - chain_depth uniform 1–5 (metadata field, does not affect DB cost but
 *     exercises JSON marshal/unmarshal at both ends)
 *   - 10 rotating tenant_id + session_id pairs
 *
 * Required env vars:
 *   BASE_URL   — defaults to http://localhost:7600
 *   API_TOKEN  — Bearer token (optional on dev stacks that skip auth)
 *
 * Run:
 *   k6 run tests/k6/wave3-outcomes-record.js
 */

import { check } from "k6";
import http from "k6/http";
import { Counter, Rate, Trend } from "k6/metrics";

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

const BASE_URL = __ENV.BASE_URL || "http://localhost:7600";

// ---------------------------------------------------------------------------
// Custom metrics
// ---------------------------------------------------------------------------

const outcomeRecordLatency = new Trend("outcome_record_latency", true);
const outcomeErrorRate = new Rate("outcome_record_error_rate");
const sloBreaches = new Counter("outcome_record_slo_breaches");

// ---------------------------------------------------------------------------
// Test data
// ---------------------------------------------------------------------------

const OUTCOME_KINDS = [
  "revenue_usd",
  "cost_saved_usd",
  "hours_saved",
  "deals_closed",
  "tickets_resolved",
  "custom",
];

const AGENT_NAMES = [
  "sales-bot",
  "support-bot",
  "finance-bot",
  "hr-bot",
  "devops-bot",
];

const TENANTS = [
  { tenant_id: "tenant-a", sessions: ["sess-a1", "sess-a2", "sess-a3"] },
  { tenant_id: "tenant-b", sessions: ["sess-b1", "sess-b2"] },
  { tenant_id: "tenant-c", sessions: ["sess-c1", "sess-c2", "sess-c3"] },
  { tenant_id: "tenant-d", sessions: ["sess-d1"] },
  { tenant_id: "tenant-e", sessions: ["sess-e1", "sess-e2"] },
  { tenant_id: "tenant-f", sessions: ["sess-f1", "sess-f2"] },
  { tenant_id: "tenant-g", sessions: ["sess-g1", "sess-g2", "sess-g3"] },
  { tenant_id: "tenant-h", sessions: ["sess-h1"] },
  { tenant_id: "tenant-i", sessions: ["sess-i1", "sess-i2"] },
  { tenant_id: "tenant-j", sessions: ["sess-j1", "sess-j2"] },
];

const UNIT_MAP = {
  revenue_usd: "usd",
  cost_saved_usd: "usd",
  hours_saved: "hours",
  deals_closed: "count",
  tickets_resolved: "count",
  custom: "units",
};

function pick(arr) {
  return arr[Math.floor(Math.random() * arr.length)];
}

function randomChainDepth() {
  // Uniform 1–5 as specified.
  return Math.floor(Math.random() * 5) + 1;
}

function randomValue(kind) {
  switch (kind) {
    case "revenue_usd":
      return Math.round(Math.random() * 50000 + 100) / 100;
    case "cost_saved_usd":
      return Math.round(Math.random() * 5000 + 10) / 100;
    case "hours_saved":
      return Math.round(Math.random() * 40 * 10) / 10;
    case "deals_closed":
      return Math.floor(Math.random() * 5) + 1;
    case "tickets_resolved":
      return Math.floor(Math.random() * 20) + 1;
    case "custom":
      return Math.round(Math.random() * 1000 * 10) / 10;
    default:
      return 1;
  }
}

function buildPayload() {
  const tenant = pick(TENANTS);
  const kind = pick(OUTCOME_KINDS);
  const chainDepth = randomChainDepth();

  return JSON.stringify({
    tenant_id: tenant.tenant_id,
    session_id: pick(tenant.sessions),
    agent_name: pick(AGENT_NAMES),
    kind: kind,
    value: randomValue(kind),
    unit: UNIT_MAP[kind],
    description: `Automated outcome attribution — chain depth ${chainDepth}`,
    metadata: {
      chain_depth: chainDepth,
      source: "k6-wave3-load-test",
    },
  });
}

// ---------------------------------------------------------------------------
// k6 options
// ---------------------------------------------------------------------------

export const options = {
  scenarios: {
    outcomes_record_sustained: {
      executor: "constant-arrival-rate",
      // 500 RPS sustained — the throughput floor from the perf budget.
      rate: 500,
      timeUnit: "1s",
      duration: "5m",
      // Pre-allocated VUs; ramp VUs handle burst headroom.
      preAllocatedVUs: 60,
      maxVUs: 200,
    },
  },
  thresholds: {
    "http_req_duration{name:outcomes-record}": ["p(95)<50"],
    outcome_record_latency: ["p(95)<50"],
    outcome_record_error_rate: ["rate<0.005"],
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
  const res = http.post(`${BASE_URL}/v1/outcomes`, buildPayload(), {
    headers: authHeaders(),
    tags: { name: "outcomes-record" },
  });

  outcomeRecordLatency.add(res.timings.duration);
  if (res.timings.duration > 50) {
    sloBreaches.add(1);
  }

  const ok = check(res, {
    "outcomes-record: status 201": (r) => r.status === 201,
    "outcomes-record: body non-empty": (r) => r.body && r.body.length > 0,
  });

  outcomeErrorRate.add(!ok ? 1 : 0);
  // No sleep: arrival-rate executor drives pacing independently.
}
