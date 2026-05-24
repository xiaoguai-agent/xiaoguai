# Xiaoguai Load Tests (k6)

Performance and resilience test suite for the Xiaoguai API.  
Written for [k6](https://k6.io/) — a developer-friendly load-testing tool.

---

## Install k6

```bash
# macOS (Homebrew)
brew install k6

# Ubuntu / Debian
sudo gpg --no-default-keyring --keyring /usr/share/keyrings/k6-archive-keyring.gpg \
     --keyserver hkp://keyserver.ubuntu.com:80 --recv-keys C5AD17C747E3415A3642D57D77C6C491D6AC1D69
echo "deb [signed-by=/usr/share/keyrings/k6-archive-keyring.gpg] https://dl.k6.io/deb stable main" \
     | sudo tee /etc/apt/sources.list.d/k6.list
sudo apt-get update && sudo apt-get install k6

# Docker (no install required)
docker pull grafana/k6
```

---

## Quick start (local dev)

```bash
# 1. Bring up the API stack
docker compose -f deploy/docker-compose.yml up -d --build

# 2. Create a session to use for chat scenarios
SESSION_ID=$(curl -s -X POST http://localhost:7600/v1/sessions \
  -H 'Content-Type: application/json' \
  -d '{"name":"k6-test"}' | jq -r '.id')

# 3. Run the CI smoke (30 s, 5 VUs) — fast sanity check
SESSION_ID=$SESSION_ID make -C scripts/loadtest smoke
```

---

## Scenarios

| File | VUs | Duration | What it tests |
|---|---|---|---|
| `chat-burst.js` | 50 | 5 min | Sustained POST /messages — LLM throughput + DB writes |
| `webhook-storm.js` | 100 | 3 min | Scheduler webhook routes — validates C15 rate limiting |
| `usage-readheavy.js` | 40 | 5 min | GET /usage + GET /sessions — read path + analytics |
| `mixed.js` | 60 | 5 min | 70 % reads / 20 % chat / 10 % admin — realistic mix |
| `spike.js` | 0 → 500 → 0 | ~3 min | Autoscaling + circuit-breaker resilience |

### Thresholds (all scenarios)

Every scenario fails the k6 exit code (non-zero) if either of these is breached:

| Metric | Threshold |
|---|---|
| `http_req_duration` | p95 < 500 ms |
| `http_req_failed` | rate < 1 % |

`spike.js` uses a wider threshold (`p95 < 2 000 ms`, `rate < 10 %`) because
the spike itself intentionally saturates the stack.

---

## Makefile targets

```bash
# From repo root:
make -C scripts/loadtest smoke                       # CI-safe, 30 s
make -C scripts/loadtest chat  SESSION_ID=<uuid>     # full chat-burst
make -C scripts/loadtest webhook \
  WEBHOOK_ROUTE_ID=<id> \
  WEBHOOK_ROUTE_TOKEN=<tok>                          # webhook-storm
make -C scripts/loadtest usage                       # usage-readheavy
make -C scripts/loadtest mixed SESSION_ID=<uuid>     # realistic mix
make -C scripts/loadtest spike SESSION_ID=<uuid>     # spike
make -C scripts/loadtest full  SESSION_ID=<uuid>     # all scenarios
```

### Environment variables

| Variable | Default | Description |
|---|---|---|
| `BASE_URL` | `http://localhost:7600` | API base URL |
| `API_TOKEN` | _(empty)_ | Bearer token. Omit on dev stacks that skip auth. |
| `SESSION_ID` | _(required for chat/mixed/spike)_ | Pre-created session UUID |
| `WEBHOOK_ROUTE_ID` | _(required for webhook-storm)_ | Webhook route ID |
| `WEBHOOK_ROUTE_TOKEN` | _(empty)_ | Webhook token (from `/v1/scheduler/webhooks`) |

---

## Pointing at staging vs production

```bash
# Staging
BASE_URL=https://staging.xiaoguai.example.com \
API_TOKEN=<staging-token> \
SESSION_ID=<staging-session> \
make -C scripts/loadtest smoke

# Production (read-only scenarios only — do not run spike against prod)
BASE_URL=https://api.xiaoguai.example.com \
API_TOKEN=<prod-token> \
make -C scripts/loadtest usage
```

**Never run `spike.js` or `full` against a production environment.**

---

## Interpreting results

k6 prints a summary table after each run.  Key metrics:

| Metric | What it means |
|---|---|
| `http_req_duration` | End-to-end latency (look at `p(90)`, `p(95)`, `p(99)`) |
| `http_req_failed` | Fraction of non-2xx responses |
| `http_reqs` | Total request count + throughput (req/s) |
| `chat_latency` | Custom trend for POST /messages specifically |
| `webhook_rate_limited` | Rate of 429 responses from webhook routes |
| `circuit_breaker_trips` | Rate of 503 responses during spike (spike.js only) |
| `slow_requests` | Count of requests that exceeded 500 ms |

A green run ends with:

```
✓ http_req_duration.........: p(95)=… < 500ms
✓ http_req_failed...........: … < 1%
```

A red threshold prints `✗` and causes k6 to exit with code 99 (CI-detectable).

---

## CI integration

The GitHub Actions workflow `.github/workflows/loadtest-smoke.yml` runs
`make smoke` automatically on pull requests that touch `crates/xiaoguai-api/`
or `crates/xiaoguai-core/`.

The full suite (`make full`) is triggered only via `workflow_dispatch` to
avoid long-running jobs blocking PRs.

---

## Grafana k6 Cloud (opt-in)

k6 can stream metrics to [Grafana k6 Cloud](https://grafana.com/products/cloud/k6/)
for persistent dashboards and trend analysis.

```bash
# Authenticate once
k6 cloud login --token <your-k6-cloud-token>

# Run a scenario in the cloud
k6 cloud scripts/loadtest/k6/scenarios/chat-burst.js

# Or stream a local run to the cloud
K6_CLOUD_TOKEN=<token> k6 run --out cloud \
  scripts/loadtest/k6/scenarios/mixed.js
```

This is entirely opt-in.  Local `make smoke` / `make full` runs do not
require a k6 Cloud account.
