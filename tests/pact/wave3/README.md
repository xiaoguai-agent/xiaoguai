# Pact consumer-driven contract tests — wave-3 API

Consumer-driven contract (CDC) tests for the xiaoguai wave-3 API surface
(HotL policies + check, outcomes, skills). Four consumers are covered:
TypeScript SDK, Python SDK, Go SDK, and chat-ui.

## What is Pact?

Pact is a consumer-driven contract testing framework. Consumers (SDK clients,
frontend components) define the interactions they expect from the provider
(xiaoguai-api). These expectations are serialised as JSON pact files. The
provider then verifies it satisfies every consumer's pact, without consumers
and providers needing to run simultaneously.

```
Consumer test  →  pact file (JSON)  →  Provider verification
    (write)                                 (read + replay)
```

This catches API drift before it reaches production: if a provider changes a
field name, a consumer's test immediately fails during provider CI.

## Directory layout

```
tests/pact/wave3/
├── README.md                   ← this file
├── provider-verify.sh          ← manual / CI provider verification runner
├── pacts/                      ← generated pact files (git-committed as source of truth)
│   ├── typescript-sdk-xiaoguai.json
│   ├── python-sdk-xiaoguai.json
│   ├── go-sdk-xiaoguai.json
│   └── chat-ui-xiaoguai.json
└── consumers/
    ├── typescript-sdk/          ← @pact-foundation/pact (PactV3)
    │   ├── package.json
    │   ├── tsconfig.json
    │   └── xiaoguai-wave3.pact.test.ts   (12 interactions)
    ├── python-sdk/              ← pact-python
    │   ├── pyproject.toml
    │   └── test_xiaoguai_wave3.py        (12 interactions)
    ├── go-sdk/                  ← pact-go v2
    │   ├── go.mod
    │   └── xiaoguai_wave3_pact_test.go   (12 interactions)
    └── chat-ui/                 ← @pact-foundation/pact (PactV3)
        ├── package.json
        ├── tsconfig.json
        └── xiaoguai-wave3.pact.test.ts   (2 interactions)
```

## Interactions summary

| # | Consumer | Method | Path | State |
|---|----------|--------|------|-------|
| 1 | all | GET | `/v1/hotl/policies` | HotL policy exists |
| 2 | all | POST | `/v1/hotl/policies` | HotL policy store is available |
| 3 | all | GET | `/v1/hotl/policies/:id` | HotL policy exists |
| 4 | all | PUT | `/v1/hotl/policies/:id` | HotL policy exists |
| 5 | all | DELETE | `/v1/hotl/policies/:id` | HotL policy exists |
| 6 | all | POST | `/v1/hotl/check` | budget within limits |
| 7 | all | POST | `/v1/outcomes` | outcome writer available |
| 8 | all | GET | `/v1/outcomes/summary?range=7d` | owner has recorded outcomes |
| 9 | all | GET | `/v1/outcomes/timeseries?range=7d` | owner has recorded outcomes |
| 10 | all | GET | `/v1/skills/installed` | owner has installed skill packs |
| 11 | all | POST | `/v1/skills/install` | pr-review in catalog |
| 12 | all | DELETE | `/v1/skills/install/:id` | installation exists |
| C1 | chat-ui | GET | `/v1/outcomes/summary?session_id=` | session has outcomes |
| C2 | chat-ui | POST | `/v1/hotl/check` | budget within limits |

> **DEC-033 single-owner:** the `tenant_id` query/body/response field was
> removed across every interaction. The implicit owner is the only principal,
> so requests no longer scope by tenant and responses no longer echo it.
> The former chat-ui interaction C3 (`GET /v1/tenants/:id/config`) was dropped
> entirely — per-tenant config does not exist under single-owner, and chat-ui
> no longer calls that endpoint.

## Running consumer tests

### Prerequisites

- **TypeScript/chat-ui**: Node 20+ with npm
- **Python SDK**: Python 3.11+ with `pip install -e ".[test]"` or `uv sync`
- **Go SDK**: Go 1.22+ and the pact-go native library (see pact-go docs)

### Generate pact files (consumer side)

```bash
# TypeScript SDK
cd tests/pact/wave3/consumers/typescript-sdk
npm install
npm run pact:test
# → writes tests/pact/wave3/pacts/typescript-sdk-xiaoguai.json

# Python SDK
cd tests/pact/wave3/consumers/python-sdk
pip install -e ".[test]"
pytest
# → writes tests/pact/wave3/pacts/python-sdk-xiaoguai.json

# Go SDK
cd tests/pact/wave3/consumers/go-sdk
go test ./... -v
# → writes tests/pact/wave3/pacts/go-sdk-xiaoguai.json

# chat-ui
cd tests/pact/wave3/consumers/chat-ui
npm install
npm run pact:test
# → writes tests/pact/wave3/pacts/chat-ui-xiaoguai.json
```

## Running provider verification

```bash
# Start the xiaoguai API (dev mode, in-memory stores):
cargo run -p xiaoguai-api -- --dev

# In another terminal:
PROVIDER_BASE_URL=http://localhost:8080 \
  tests/pact/wave3/provider-verify.sh
```

### Expected output (pre-bridge state)

Interactions are backed by the in-memory store implementations wired in
`--dev` mode (single-owner SQLite). All interactions (healthz routing, bearer
auth middleware, error envelopes, HotL/outcomes/skills handlers) should verify
successfully.

```
12 interactions (typescript-sdk), 0 failures
2 interactions  (chat-ui),       0 failures
```

## Adding a new consumer

1. Create a directory under `consumers/<name>/`.
2. Write interactions using PactV3 (TS/Go) or pact-python.
3. Configure the pact file output dir to point at `../../pacts/` (relative to
   your consumer dir) so all pacts land in the shared `pacts/` folder.
4. Add your consumer to `.github/workflows/pact-consumer-tests.yml`.
5. Commit the generated pact JSON file — it is the source of truth.

## Pactflow (brokered) mode

For a shared broker (recommended for teams):

```bash
export PACTFLOW_BASE_URL=https://your-org.pactflow.io
export PACTFLOW_TOKEN=<read-write token>

# Publish pacts after consumer CI
pact-broker publish tests/pact/wave3/pacts/ \
  --consumer-app-version "$(git rev-parse --short HEAD)" \
  --branch "$(git branch --show-current)" \
  --broker-base-url "$PACTFLOW_BASE_URL" \
  --broker-token "$PACTFLOW_TOKEN"

# Verify and publish results
pact-provider-verifier \
  --provider xiaoguai \
  --provider-base-url http://localhost:8080 \
  --pact-broker-base-url "$PACTFLOW_BASE_URL" \
  --broker-token "$PACTFLOW_TOKEN" \
  --publish-verification-results true \
  --provider-app-version "$(git rev-parse --short HEAD)"
```

## Pact spec version

All consumers and the provider use **Pact spec v3**, which supports:
- Provider states with parameters (`GivenWithParameter`)
- Multiple interactions in a single test
- Regex, `Like`, `EachLike`, and `Term` matchers
- Matching rules on request query strings and headers
