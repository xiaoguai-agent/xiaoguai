/**
 * tests/k6/wave3-outcomes-read.js
 *
 * Wave-3 load test: GET /v1/outcomes/summary + GET /v1/outcomes/timeseries
 *
 * Two independent scenarios with separate SLO thresholds:
 *
 *   outcomes_summary    — 100 RPS sustained, p95 < 200 ms
 *     (GROUP BY per kind over date range; daily-bucket pre-agg amortises cost)
 *   outcomes_timeseries — 50 RPS sustained, p95 < 500 ms
 *     (Day-granularity scan up to 30 rows × 6 kinds; bounded O(days × kinds))
 *
 * Both use `constant-arrival-rate` to achieve the rated throughput floors
 * from perf-budget-wave3.md §2 independently.
 *
 * Required env vars:
 *   BASE_URL   — defaults to http://localhost:7600
 *   API_TOKEN  — Bearer token (optional on dev stacks that skip auth)
 *
 * Run:
 *   k6 run tests/k6/wave3-outcomes-read.js
 */

import { check } from "k6";
import http from "k6/http";
import { Counter, Rate, Trend } from "k6/metrics";

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

const BASE_URL = __ENV.BASE_URL || "http://localhost:7600";

// ---------------------------------------------------------------------------
// Custom metrics — one set per endpoint so thresholds can be name-scoped.
// ---------------------------------------------------------------------------

const summaryLatency = new Trend("outcomes_summary_latency", true);
const timeseriesLatency = new Trend("outcomes_timeseries_latency", true);
const summaryErrorRate = new Rate("outcomes_summary_error_rate");
const timeseriesErrorRate = new Rate("outcomes_timeseries_error_rate");
const summarySloBreach = new Counter("outcomes_summary_slo_breaches");
const timeseriesSloBreach = new Counter("outcomes_timeseries_slo_breaches");

// ---------------------------------------------------------------------------
// Test data
// ---------------------------------------------------------------------------

const TENANT_IDS = [
  "tenant-a",
  "tenant-b",
  "tenant-c",
  "tenant-d",
  "tenant-e",
  "tenant-f",
  "tenant-g",
  "tenant-h",
  "tenant-i",
  "tenant-j",
];

// Date windows — vary to stress index scans at different granularities.
const SUMMARY_WINDOWS = [
  { since: "2026-01-01", until: "2026-01-31" }, // 30 days
  { since: "2026-02-01", until: "2026-02-28" }, // 28 days
  { since: "2026-03-01", until: "2026-03-31" }, // 31 days
  { since: "2026-04-01", until: "2026-04-30" }, // 30 days
  { since: "2026-05-01", until: "2026-05-25" }, // recent partial month
];

const TIMESERIES_WINDOWS = [
  { since: "2026-03-01", until: "2026-05-25", granularity: "day" }, // ~85 days → bounded by 30-row cap per kind
  { since: "2026-04-01", until: "2026-05-25", granularity: "day" }, // ~55 days
  { since: "2026-05-01", until: "2026-05-25", granularity: "day" }, // ~25 days
  { since: "2026-01-01", until: "2026-03-31", granularity: "day" }, // Q1
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
// k6 options — two named scenarios, separate thresholds via name tags.
// ---------------------------------------------------------------------------

export const options = {
  scenarios: {
    // ---- summary: 100 RPS, p95 < 200 ms ----
    outcomes_summary: {
      executor: "constant-arrival-rate",
      rate: 100,
      timeUnit: "1s",
      duration: "5m",
      preAllocatedVUs: 25,
      maxVUs: 80,
      exec: "runSummary",
    },
    // ---- timeseries: 50 RPS, p95 < 500 ms ----
    outcomes_timeseries: {
      executor: "constant-arrival-rate",
      rate: 50,
      timeUnit: "1s",
      duration: "5m",
      preAllocatedVUs: 15,
      maxVUs: 50,
      exec: "runTimeseries",
    },
  },
  thresholds: {
    // Summary SLO — scoped via name tag.
    "http_req_duration{name:outcomes-summary}": ["p(95)<200"],
    outcomes_summary_latency: ["p(95)<200"],
    outcomes_summary_error_rate: ["rate<0.005"],

    // Timeseries SLO — scoped via name tag.
    "http_req_duration{name:outcomes-timeseries}": ["p(95)<500"],
    outcomes_timeseries_latency: ["p(95)<500"],
    outcomes_timeseries_error_rate: ["rate<0.005"],

    // Combined safety net across both scenarios.
    http_req_failed: ["rate<0.01"],
  },
};

// ---------------------------------------------------------------------------
// Scenario exec functions
// ---------------------------------------------------------------------------

export function runSummary() {
  const tenant = pick(TENANT_IDS);
  const window = pick(SUMMARY_WINDOWS);

  const url =
    `${BASE_URL}/v1/outcomes/summary` +
    `?tenant_id=${encodeURIComponent(tenant)}` +
    `&since=${window.since}` +
    `&until=${window.until}`;

  const res = http.get(url, {
    headers: authHeaders(),
    tags: { name: "outcomes-summary" },
  });

  summaryLatency.add(res.timings.duration);
  if (res.timings.duration > 200) {
    summarySloBreach.add(1);
  }

  const ok = check(res, {
    "outcomes-summary: status 200": (r) => r.status === 200,
    "outcomes-summary: body non-empty": (r) => r.body && r.body.length > 0,
  });

  summaryErrorRate.add(!ok ? 1 : 0);
}

export function runTimeseries() {
  const tenant = pick(TENANT_IDS);
  const window = pick(TIMESERIES_WINDOWS);

  const url =
    `${BASE_URL}/v1/outcomes/timeseries` +
    `?tenant_id=${encodeURIComponent(tenant)}` +
    `&since=${window.since}` +
    `&until=${window.until}` +
    `&granularity=${window.granularity}`;

  const res = http.get(url, {
    headers: authHeaders(),
    tags: { name: "outcomes-timeseries" },
  });

  timeseriesLatency.add(res.timings.duration);
  if (res.timings.duration > 500) {
    timeseriesSloBreach.add(1);
  }

  const ok = check(res, {
    "outcomes-timeseries: status 200": (r) => r.status === 200,
    "outcomes-timeseries: body non-empty": (r) => r.body && r.body.length > 0,
  });

  timeseriesErrorRate.add(!ok ? 1 : 0);
}

// Default is unused when exec: is specified per-scenario,
// but k6 requires an export default for files using per-scenario exec.
export default function () {}
