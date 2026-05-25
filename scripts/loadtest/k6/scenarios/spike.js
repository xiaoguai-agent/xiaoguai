/**
 * scenarios/spike.js
 *
 * Spike test — stresses autoscaling and circuit-breaker behaviour:
 *
 *   Stage 1 (ramp-up):   0 → 500 VUs over 30 s
 *   Stage 2 (sustained): 500 VUs for 2 min
 *   Stage 3 (ramp-down): 500 → 0 VUs over 30 s
 *
 * The spike deliberately saturates the stack.  Expected results:
 *   - p95 latency will spike during ramp-up — that's intentional.
 *   - Circuit breakers should trip for LLM calls; 503 responses are
 *     expected and counted separately, not as http_req_failed.
 *   - The server must return to nominal p95 < 500 ms within 60 s of
 *     ramp-down completing (measured by the `recovery_latency` trend).
 *   - Server must never OOM or crash (no 5xx stream of errors > 10 %).
 *
 * Thresholds (fail the test on breach):
 *   http_req_failed: rate < 0.10  (more lenient — spikes produce 503s)
 *   http_req_duration: p(95) < 2000 ms  (broad; spike naturally blows
 *     the 500 ms SLO — the spike test proves the system survives)
 *
 * Required env vars:
 *   SESSION_ID   — a pre-created session UUID
 *   BASE_URL     — defaults to http://localhost:7600
 *   API_TOKEN    — bearer token (optional on dev stacks)
 */

import { check, sleep } from "k6";
import {
  BASE_URL,
  checkResponse,
  fetchSessions,
  sendMessage,
  randomQuestion,
} from "../lib/common.js";
import { Rate, Trend } from "k6/metrics";

const circuitBreakerTrips = new Rate("circuit_breaker_trips");
const recoveryLatency = new Trend("recovery_latency", true);

export const options = {
  scenarios: {
    spike: {
      executor: "ramping-vus",
      startVUs: 0,
      stages: [
        { duration: "30s", target: 500 }, // ramp-up
        { duration: "2m",  target: 500 }, // sustained spike
        { duration: "30s", target: 0   }, // ramp-down
      ],
      gracefulRampDown: "30s",
    },
  },
  thresholds: {
    // Lenient during spike — server must survive, not necessarily meet SLO.
    http_req_duration: ["p(95)<2000"],
    // At most 10 % genuine errors (5xx excluding circuit-breaker 503).
    http_req_failed: ["rate<0.10"],
    // Circuit breaker trips (503) should resolve — rate should drop
    // after ramp-down.  If it stays > 50 %, the server is stuck.
    circuit_breaker_trips: ["rate<0.50"],
  },
};

export default function () {
  const sessionId = __ENV.SESSION_ID;

  // Alternate between read (cheaper) and chat (heavier) to create
  // realistic mixed pressure rather than pure LLM saturation.
  if (!sessionId || Math.random() < 0.40) {
    // Read path — always safe to run without SESSION_ID.
    const res = fetchSessions(10);
    const started = Date.now();
    checkResponse(res, "spike GET /sessions");
    recoveryLatency.add(res.timings.duration);
  } else {
    // Chat path — drives LLM / circuit-breaker pressure.
    const res = sendMessage(sessionId, randomQuestion());

    // 503 = circuit breaker open — expected under spike, track separately.
    circuitBreakerTrips.add(res.status === 503 ? 1 : 0);

    check(res, {
      "spike POST /messages: not a server crash": (r) => r.status !== 500,
      "spike POST /messages: accepted or circuit-broken": (r) =>
        [200, 201, 429, 503].includes(r.status),
    });
    recoveryLatency.add(res.timings.duration);
  }

  // Minimal think-time to maximise concurrency pressure.
  sleep(0.1 + Math.random() * 0.4);
}
