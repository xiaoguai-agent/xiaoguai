/**
 * lib/common.js — shared helpers for all k6 load-test scenarios.
 *
 * Environment variables (set before running k6):
 *   BASE_URL          API base, e.g. http://localhost:7600  (default: http://localhost:7600)
 *   API_TOKEN         Bearer token obtained from POST /v1/auth/token
 *   SESSION_ID        A pre-created session UUID to use for chat scenarios
 *   WEBHOOK_ROUTE_ID  A pre-created webhook route ID for scheduler tests
 */

import { check } from "k6";
import http from "k6/http";
import { Counter, Rate, Trend } from "k6/metrics";

// ---------------------------------------------------------------------------
// Base URL + auth helpers
// ---------------------------------------------------------------------------

export const BASE_URL = __ENV.BASE_URL || "http://localhost:7600";

/**
 * Return HTTP headers with Bearer auth when API_TOKEN is set.
 * Falls back to no Authorization header (dev stacks that skip auth).
 */
export function authHeaders(extra = {}) {
  const headers = { "Content-Type": "application/json", ...extra };
  if (__ENV.API_TOKEN) {
    headers["Authorization"] = `Bearer ${__ENV.API_TOKEN}`;
  }
  return headers;
}

// ---------------------------------------------------------------------------
// Custom metrics (shared across scenarios)
// ---------------------------------------------------------------------------

/** Count of requests that breached the 500 ms SLO. */
export const slowRequests = new Counter("slow_requests");

/** End-to-end latency for POST /messages (excludes streaming read). */
export const chatLatency = new Trend("chat_latency", true);

/** End-to-end latency for GET /usage. */
export const usageLatency = new Trend("usage_latency", true);

/** Rate of 429 (rate-limited) responses from webhook routes. */
export const webhookRateLimited = new Rate("webhook_rate_limited");

// ---------------------------------------------------------------------------
// Utility: assert + track slow requests
// ---------------------------------------------------------------------------

/**
 * Run standard checks on a response and increment slowRequests when
 * the request took more than 500 ms.
 *
 * @param {object} res   k6 Response object
 * @param {string} label Human-readable label for check names
 * @param {number[]} okStatuses HTTP status codes treated as success (default [200, 201])
 * @returns {boolean} true when all checks passed
 */
export function checkResponse(res, label, okStatuses = [200, 201]) {
  const ok = check(res, {
    [`${label}: status ok`]: (r) => okStatuses.includes(r.status),
    [`${label}: body non-empty`]: (r) => r.body && r.body.length > 0,
  });
  if (res.timings.duration > 500) {
    slowRequests.add(1);
  }
  return ok;
}

// ---------------------------------------------------------------------------
// Utility: POST /v1/sessions/:id/messages
// ---------------------------------------------------------------------------

/**
 * Send one chat message to a session and return the response.
 *
 * @param {string} sessionId  UUID of the session
 * @param {string} content    Message text
 */
export function sendMessage(sessionId, content) {
  const url = `${BASE_URL}/v1/sessions/${sessionId}/messages`;
  const payload = JSON.stringify({ role: "user", content });
  const res = http.post(url, payload, { headers: authHeaders() });
  chatLatency.add(res.timings.duration);
  return res;
}

// ---------------------------------------------------------------------------
// Utility: GET /v1/usage
// ---------------------------------------------------------------------------

export function fetchUsage(params = "") {
  const url = `${BASE_URL}/v1/usage${params ? "?" + params : ""}`;
  const res = http.get(url, { headers: authHeaders() });
  usageLatency.add(res.timings.duration);
  return res;
}

// ---------------------------------------------------------------------------
// Utility: GET /v1/sessions
// ---------------------------------------------------------------------------

export function fetchSessions(limit = 20) {
  const url = `${BASE_URL}/v1/sessions?limit=${limit}`;
  return http.get(url, { headers: authHeaders() });
}

// ---------------------------------------------------------------------------
// Utility: POST scheduler webhook route
// ---------------------------------------------------------------------------

/**
 * Hit a scheduler webhook route.
 *
 * @param {string} routeId    The webhook route ID
 * @param {string} token      Webhook token (from WEBHOOK_ROUTE_TOKEN env or empty)
 * @param {object} body       JSON body to POST
 */
export function triggerWebhook(routeId, token, body = {}) {
  const url = `${BASE_URL}/v1/scheduler/webhooks/${routeId}`;
  const headers = { "Content-Type": "application/json" };
  if (token) {
    headers["X-Webhook-Token"] = token;
  }
  const res = http.post(url, JSON.stringify(body), { headers });
  webhookRateLimited.add(res.status === 429 ? 1 : 0);
  return res;
}

// ---------------------------------------------------------------------------
// Utility: random payload helpers
// ---------------------------------------------------------------------------

const SAMPLE_QUESTIONS = [
  "Summarise the last 5 messages.",
  "What is 2 + 2?",
  "Tell me a one-sentence joke.",
  "List three capitals of European countries.",
  "What time is it in Tokyo?",
  "Translate 'hello' to Spanish.",
  "Name one use-case for a scheduler.",
  "Explain REST in one sentence.",
];

export function randomQuestion() {
  return SAMPLE_QUESTIONS[Math.floor(Math.random() * SAMPLE_QUESTIONS.length)];
}
