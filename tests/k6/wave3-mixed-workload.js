/**
 * tests/k6/wave3-mixed-workload.js
 *
 * Wave-3 load test: realistic mixed workload across all wave-3 endpoints.
 *
 * Traffic split (matches perf-budget-wave3.md §1.3 + observed production mix):
 *   70 % — POST /v1/hotl/check        (high-frequency enforcement checks)
 *   20 % — POST /v1/outcomes          (attribution writes)
 *   5  % — GET  /v1/outcomes/summary  (admin dashboard reads)
 *   3  % — GET  /v1/outcomes/timeseries (chart reads)
 *   2  % — GET  /v1/skills/installed  (skill pane)
 *
 * All SLOs must hold simultaneously — this is the integration gate.
 *
 * Load profile:
 *   150 VUs constant (maps to ~750–900 effective RPS with minimal sleep,
 *   covering the blended throughput mix above).
 *
 * Threshold rationale:
 *   - hotl-check p95 < 25 ms (the tightest SLO dominates)
 *   - outcomes-record p95 < 50 ms
 *   - read endpoints p95 < 500 ms (timeseries; summary < 200 ms also asserted)
 *   - Global error rate < 0.5 % (perf-budget §4.2 paging condition)
 *
 * Required env vars:
 *   BASE_URL   — defaults to http://localhost:7600
 *   API_TOKEN  — Bearer token (optional on dev stacks that skip auth)
 *
 * Run:
 *   k6 run tests/k6/wave3-mixed-workload.js
 */

import { check, sleep } from "k6";
import http from "k6/http";
import { Counter, Rate, Trend } from "k6/metrics";

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

const BASE_URL = __ENV.BASE_URL || "http://localhost:7600";

// ---------------------------------------------------------------------------
// Custom metrics — per-endpoint for clear SLO attribution in reports.
// ---------------------------------------------------------------------------

const hotlLatency = new Trend("mixed_hotl_latency", true);
const outcomeWriteLatency = new Trend("mixed_outcome_write_latency", true);
const summaryLatency = new Trend("mixed_summary_latency", true);
const timeseriesLatency = new Trend("mixed_timeseries_latency", true);
const skillsLatency = new Trend("mixed_skills_latency", true);

const mixedErrorRate = new Rate("mixed_error_rate");
const sloViolations = new Counter("mixed_slo_violations");

// ---------------------------------------------------------------------------
// Test data helpers (shared across tiers)
// ---------------------------------------------------------------------------

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
];

const SCOPES = ["llm_call", "email_send", "webhook_invoke", "tool_call"];

const OUTCOME_KINDS = [
  "revenue_usd",
  "cost_saved_usd",
  "hours_saved",
  "deals_closed",
  "tickets_resolved",
  "custom",
];

const SKILL_TENANTS = [
  "tenant-skills-a",
  "tenant-skills-b",
  "tenant-skills-c",
  "tenant-skills-d",
  "tenant-skills-e",
];

const DATE_WINDOWS = [
  { since: "2026-04-01", until: "2026-05-25" },
  { since: "2026-03-01", until: "2026-05-25" },
  { since: "2026-05-01", until: "2026-05-25" },
];

function pick(arr) {
  return arr[Math.floor(Math.random() * arr.length)];
}

function authHeaders() {
  const h = { "Content-Type": "application/json" };
  if (__ENV.API_TOKEN) {
    h["Authorization"] = `Bearer ${__ENV.API_TOKEN}`;
  }
  return h;
}

// ---------------------------------------------------------------------------
// Tier implementations
// ---------------------------------------------------------------------------

function doHotlCheck() {
  const res = http.post(
    `${BASE_URL}/v1/hotl/check`,
    JSON.stringify({
      tenant_id: pick(TENANT_IDS),
      scope: pick(SCOPES),
      amount: Math.random() < 0.8 ? 1.0 : Math.round(Math.random() * 100) / 1000,
    }),
    { headers: authHeaders(), tags: { name: "mixed-hotl-check" } }
  );

  hotlLatency.add(res.timings.duration);
  if (res.timings.duration > 25) sloViolations.add(1);

  const ok = check(res, {
    "mixed hotl-check: 2xx or 402/429": (r) =>
      [200, 201, 402, 429].includes(r.status),
  });
  mixedErrorRate.add(!ok ? 1 : 0);
}

function doOutcomeRecord() {
  const kind = pick(OUTCOME_KINDS);
  const chainDepth = Math.floor(Math.random() * 5) + 1;

  const unitMap = {
    revenue_usd: "usd",
    cost_saved_usd: "usd",
    hours_saved: "hours",
    deals_closed: "count",
    tickets_resolved: "count",
    custom: "units",
  };

  const res = http.post(
    `${BASE_URL}/v1/outcomes`,
    JSON.stringify({
      tenant_id: `tenant-${String.fromCharCode(97 + Math.floor(Math.random() * 10))}`,
      session_id: `sess-${Math.floor(Math.random() * 50) + 1}`,
      agent_name: pick(["sales-bot", "support-bot", "finance-bot"]),
      kind,
      value: Math.round(Math.random() * 10000) / 100,
      unit: unitMap[kind],
      description: `Mixed workload attribution — chain depth ${chainDepth}`,
      metadata: { chain_depth: chainDepth, source: "k6-mixed" },
    }),
    { headers: authHeaders(), tags: { name: "mixed-outcomes-record" } }
  );

  outcomeWriteLatency.add(res.timings.duration);
  if (res.timings.duration > 50) sloViolations.add(1);

  const ok = check(res, {
    "mixed outcomes-record: 201": (r) => r.status === 201,
  });
  mixedErrorRate.add(!ok ? 1 : 0);
}

function doSummaryRead() {
  const tenant = `tenant-${String.fromCharCode(97 + Math.floor(Math.random() * 10))}`;
  const w = pick(DATE_WINDOWS);

  const res = http.get(
    `${BASE_URL}/v1/outcomes/summary?tenant_id=${encodeURIComponent(tenant)}&since=${w.since}&until=${w.until}`,
    { headers: authHeaders(), tags: { name: "mixed-outcomes-summary" } }
  );

  summaryLatency.add(res.timings.duration);
  if (res.timings.duration > 200) sloViolations.add(1);

  const ok = check(res, {
    "mixed outcomes-summary: 200": (r) => r.status === 200,
  });
  mixedErrorRate.add(!ok ? 1 : 0);
}

function doTimeseriesRead() {
  const tenant = `tenant-${String.fromCharCode(97 + Math.floor(Math.random() * 10))}`;
  const w = pick(DATE_WINDOWS);

  const res = http.get(
    `${BASE_URL}/v1/outcomes/timeseries?tenant_id=${encodeURIComponent(tenant)}&since=${w.since}&until=${w.until}&granularity=day`,
    { headers: authHeaders(), tags: { name: "mixed-outcomes-timeseries" } }
  );

  timeseriesLatency.add(res.timings.duration);
  if (res.timings.duration > 500) sloViolations.add(1);

  const ok = check(res, {
    "mixed outcomes-timeseries: 200": (r) => r.status === 200,
  });
  mixedErrorRate.add(!ok ? 1 : 0);
}

function doSkillsList() {
  const res = http.get(
    `${BASE_URL}/v1/skills/installed?tenant=${encodeURIComponent(pick(SKILL_TENANTS))}`,
    { headers: authHeaders(), tags: { name: "mixed-skills-installed" } }
  );

  skillsLatency.add(res.timings.duration);
  if (res.timings.duration > 100) sloViolations.add(1);

  const ok = check(res, {
    "mixed skills-installed: 200": (r) => r.status === 200,
  });
  mixedErrorRate.add(!ok ? 1 : 0);
}

// ---------------------------------------------------------------------------
// k6 options
// ---------------------------------------------------------------------------

export const options = {
  scenarios: {
    mixed_wave3: {
      executor: "constant-vus",
      vus: 150,
      duration: "7m",
    },
  },
  thresholds: {
    // Per-endpoint SLOs via name tags.
    "http_req_duration{name:mixed-hotl-check}": ["p(95)<25"],
    "http_req_duration{name:mixed-outcomes-record}": ["p(95)<50"],
    "http_req_duration{name:mixed-outcomes-summary}": ["p(95)<200"],
    "http_req_duration{name:mixed-outcomes-timeseries}": ["p(95)<500"],
    "http_req_duration{name:mixed-skills-installed}": ["p(95)<100"],

    // Custom trend thresholds (mirrors above for dashboard clarity).
    mixed_hotl_latency: ["p(95)<25"],
    mixed_outcome_write_latency: ["p(95)<50"],
    mixed_summary_latency: ["p(95)<200"],
    mixed_timeseries_latency: ["p(95)<500"],
    mixed_skills_latency: ["p(95)<100"],

    // Global error gate — no SLO violations allowed.
    mixed_error_rate: ["rate<0.005"],
    http_req_failed: ["rate<0.005"],

    // SLO violation counter: 0 breaches in a passing run.
    mixed_slo_violations: ["count<1"],
  },
};

// ---------------------------------------------------------------------------
// Default function (VU loop)
// ---------------------------------------------------------------------------

export default function () {
  const roll = Math.random();

  if (roll < 0.70) {
    // 70 %: hotl enforcement check (most frequent inner-loop call).
    doHotlCheck();

  } else if (roll < 0.90) {
    // 20 %: outcome attribution write.
    doOutcomeRecord();
    sleep(0.05 + Math.random() * 0.1);

  } else if (roll < 0.95) {
    // 5 %: outcomes summary dashboard read.
    doSummaryRead();
    sleep(0.1 + Math.random() * 0.2);

  } else if (roll < 0.98) {
    // 3 %: outcomes timeseries chart read.
    doTimeseriesRead();
    sleep(0.1 + Math.random() * 0.3);

  } else {
    // 2 %: skills installed list.
    doSkillsList();
    sleep(0.05 + Math.random() * 0.15);
  }
}
