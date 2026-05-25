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
        └── xiaoguai-wave3.pact.test.ts   (3 interactions)
```

## Interactions summary

| # | Consumer | Method | Path | State |
|---|----------|--------|------|-------|
| 1 | all | GET | `/v1/hotl/policies?tenant_id=` | tenant has one HotL policy |
| 2 | all | POST | `/v1/hotl/policies` | HotL policy store is available |
| 3 | all | GET | `/v1/hotl/policies/:id` | HotL policy exists |
| 4 | all | PUT | `/v1/hotl/policies/:id` | HotL policy exists |
| 5 | all | DELETE | `/v1/hotl/policies/:id` | HotL policy exists |
| 6 | all | POST | `/v1/hotl/check` | budget within limits |
| 7 | all | POST | `/v1/outcomes` | outcome writer available |
| 8 | all | GET | `/v1/outcomes/summary?range=7d` | tenant has recorded outcomes |
| 9 | all | GET | `/v1/outcomes/timeseries?range=7d` | tenant has recorded outcomes |
| 10 | all | GET | `/v1/skills/installed?tenant_id=` | tenant has installed skill packs |
| 11 | all | POST | `/v1/skills/install` | pr-review in catalog |
| 12 | all | DELETE | `/v1/skills/install/:id` | installation exists |
| C1 | chat-ui | GET | `/v1/outcomes/summary?session_id=` | session has outcomes |
| C2 | chat-ui | POST | `/v1/hotl/check` | budget within limits |
| **C3** | **chat-ui** | **GET** | **`/v1/tenants/:id/config`** | **GAP — not implemented** |

### Contract gap: `GET /v1/tenants/:id/config`

Interaction C3 (`GET /v1/tenants/:id/config`) is consumed by chat-ui's
`AiDisclosureBanner` component, which renders a configurable disclosure
notice required for EU AI Act / enterprise compliance. The component
expects:

```json
{
  "tenant_id": "<uuid>",
  "ai_disclosure_banner": {
    "enabled": true,
    "text": "This assistant is powered by AI. Responses may not be accurate."
  }
}
```

The route is **not mounted** in `crates/xiaoguai-api/src/routes/mod.rs`.
Provider verification will fail on this interaction until the endpoint is
implemented. This failure is intentional — Pact has surfaced the gap.

**Resolution** (wave-4):
1. Add `GET /v1/tenants/:id/config` handler in `crates/xiaoguai-api/src/routes/tenants.rs`
2. Back it with `AppState.tenant_config_store` (new store, similar pattern to `hotl_policy_store`)
3. Remove the `[PROVIDER GAP]` label from the chat-ui test description
4. Re-run provider verification to confirm passing

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

Interactions backed by `PgHotlPolicyStore`, `PgOutcomeRecorder`, and
`PgSkillPackRepository` will return `503 Service Unavailable` until those
store bridges land. The `ai_disclosure_banner` endpoint will return `404`
until implemented. All other interactions (healthz routing, bearer auth
middleware, error envelopes) should verify successfully.

```
12 interactions (typescript-sdk), 0 failures   # hotl/check, outcomes record
...
1 failure  (chat-ui → GET /v1/tenants/:id/config)  ← expected, see gap above
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
