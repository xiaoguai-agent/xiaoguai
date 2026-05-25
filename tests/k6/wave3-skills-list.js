/**
 * tests/k6/wave3-skills-list.js
 *
 * Wave-3 load test: GET /v1/skills/installed
 *
 * SLO target: p95 < 100 ms at 200 RPS sustained.
 * (perf-budget-wave3.md §2 — list from `installed_skill_packs`;
 * N typically < 50 per tenant; OnceLock-cached catalog parse.)
 *
 * Light read load — uses `constant-arrival-rate` at 200 RPS.
 * No write operations; safe to run against staging or read replicas.
 *
 * Required env vars:
 *   BASE_URL   — defaults to http://localhost:7600
 *   API_TOKEN  — Bearer token (optional on dev stacks that skip auth)
 *
 * Run:
 *   k6 run tests/k6/wave3-skills-list.js
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

const skillsListLatency = new Trend("skills_list_latency", true);
const skillsErrorRate = new Rate("skills_list_error_rate");
const sloBreaches = new Counter("skills_list_slo_breaches");

// ---------------------------------------------------------------------------
// Test data
// ---------------------------------------------------------------------------

// Tenants with varying pack install counts to exercise the query at
// different cardinalities (< 5, ~10, ~30, ~50 packs).
const TENANT_IDS = [
  "tenant-skills-a",  // light: 1–4 packs
  "tenant-skills-b",  // light: 1–4 packs
  "tenant-skills-c",  // medium: ~10 packs
  "tenant-skills-d",  // medium: ~10 packs
  "tenant-skills-e",  // heavy: ~30 packs
  "tenant-skills-f",  // heavy: ~30 packs
  "tenant-skills-g",  // max: ~50 packs
  "tenant-skills-h",  // max: ~50 packs
  "tenant-skills-i",  // empty: 0 packs
  "tenant-skills-j",  // empty: 0 packs
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
// k6 options
// ---------------------------------------------------------------------------

export const options = {
  scenarios: {
    skills_list_sustained: {
      executor: "constant-arrival-rate",
      rate: 200,
      timeUnit: "1s",
      duration: "5m",
      preAllocatedVUs: 30,
      maxVUs: 100,
    },
  },
  thresholds: {
    // Primary SLO — p95 < 100 ms, scoped to this endpoint by name tag.
    "http_req_duration{name:skills-installed}": ["p(95)<100"],
    skills_list_latency: ["p(95)<100"],
    // Error budget per perf-budget §4.2.
    skills_list_error_rate: ["rate<0.005"],
    http_req_failed: ["rate<0.01"],
  },
};

// ---------------------------------------------------------------------------
// Default function (VU loop)
// ---------------------------------------------------------------------------

export default function () {
  const tenant = pick(TENANT_IDS);

  const res = http.get(
    `${BASE_URL}/v1/skills/installed?tenant=${encodeURIComponent(tenant)}`,
    {
      headers: authHeaders(),
      tags: { name: "skills-installed" },
    }
  );

  skillsListLatency.add(res.timings.duration);
  if (res.timings.duration > 100) {
    sloBreaches.add(1);
  }

  const ok = check(res, {
    "skills-installed: status 200": (r) => r.status === 200,
    // Response is a JSON array (may be empty for tenants with 0 packs).
    "skills-installed: body is JSON array": (r) => {
      if (!r.body || r.body.length === 0) return false;
      try {
        const parsed = JSON.parse(r.body);
        return Array.isArray(parsed);
      } catch (_) {
        return false;
      }
    },
  });

  skillsErrorRate.add(!ok ? 1 : 0);
  // No sleep: arrival-rate executor drives pacing.
}
