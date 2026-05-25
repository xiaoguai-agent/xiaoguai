# @xiaoguai/client

TypeScript SDK for the [xiaoguai](https://github.com/xiaoguai/xiaoguai) REST API (wave-3 endpoints).

Works in **Node.js 18+** and **modern browsers** (uses the native `fetch` API).

## Install

```bash
npm install @xiaoguai/client
# or
pnpm add @xiaoguai/client
```

No runtime dependencies.

## Quickstart

```ts
import { XiaoguaiClient } from "@xiaoguai/client";

const client = new XiaoguaiClient({
  baseUrl: "http://localhost:8080",
  token: "my-bearer-token",       // omit when auth is disabled
  timeout: 30_000,                 // ms, optional (default 30 s)
});

// --- HotL boundary policies ---
const policies = await client.listHotlPolicies({ tenant_id: "my-tenant-uuid" });

const policy = await client.createHotlPolicy({
  tenant_id: "my-tenant-uuid",
  scope: "llm_call",
  window_seconds: 3600,
  max_count: 100,
  escalate_to: "ops@example.com",
});

await client.deleteHotlPolicy(policy.id);

// --- Outcome telemetry ---
await client.recordOutcome({
  tenant_id: "my-tenant",
  agent_name: "sales-bot",
  kind: "revenue_usd",
  value: 1200.0,
  description: "Closed deal #789",
});

const summary = await client.outcomesSummary({ tenant_id: "my-tenant", range: "30d" });
const timeseries = await client.outcomesTimeseries({ tenant_id: "my-tenant", range: "7d" });

// --- Skill pack marketplace ---
const catalog = await client.listSkillCatalog();
const installed = await client.installSkill({ tenant_id: "my-tenant", pack_slug: "rag-legal" });
await client.uninstallSkill(installed.id);
const myPacks = await client.listInstalledSkills("my-tenant");
```

## Auth

Pass a bearer token via the `token` constructor option. The token is sent as `Authorization: Bearer <token>` on every request.

```ts
const client = new XiaoguaiClient({ baseUrl: "...", token: process.env.XIAOGUAI_TOKEN });
```

## Error handling

All non-2xx responses throw an error from the `XiaoguaiError` hierarchy:

| Error class | HTTP status |
|---|---|
| `AuthError` | 401 |
| `ForbiddenError` | 403 |
| `NotFoundError` | 404 |
| `ConflictError` | 409 |
| `ValidationError` | 400, 422 |
| `RateLimitError` | 429 (includes `retryAfter?: number`) |
| `ServerError` | 5xx |
| `HttpError` | any other non-2xx |

```ts
import { NotFoundError, RateLimitError } from "@xiaoguai/client";

try {
  await client.deleteHotlPolicy("unknown-id");
} catch (e) {
  if (e instanceof NotFoundError) {
    console.log("policy not found");
  } else if (e instanceof RateLimitError) {
    console.log(`retry after ${e.retryAfter}s`);
  }
}
```

## Methods reference

### HotL policies (`v1.2.3`)

| Method | Description | Server endpoint |
|---|---|---|
| `listHotlPolicies(params)` | List policies for a tenant | `GET /v1/hotl/policies` |
| `createHotlPolicy(req)` | Create a policy | `POST /v1/hotl/policies` |
| `deleteHotlPolicy(id)` | Delete a policy | `DELETE /v1/hotl/policies/:id` |
| `getHotlPolicy(id)` | Get by ID | not yet implemented server-side |
| `updateHotlPolicy(id, updates)` | Update a policy | not yet implemented server-side |
| `checkHotl(scope, amount)` | Pre-flight budget check | not yet implemented server-side |

### Outcomes (`v1.2.4`)

| Method | Description | Server endpoint |
|---|---|---|
| `recordOutcome(req)` | Record a business outcome | `POST /v1/outcomes` |
| `outcomesSummary(params)` | Aggregated ROI summary | `GET /v1/outcomes/summary` |
| `outcomesTimeseries(params)` | Daily time-series | `GET /v1/outcomes/timeseries` |
| `listOutcomes()` | Raw list | not yet implemented server-side |

### Skills (`v1.2.28`)

| Method | Description | Server endpoint |
|---|---|---|
| `listSkillCatalog()` | Built-in catalog (public) | `GET /v1/skills/catalog` |
| `listInstalledSkills(tenantId?)` | Installed packs for tenant | `GET /v1/skills/installed` |
| `installSkill(req)` | Install a pack | `POST /v1/skills/install` |
| `uninstallSkill(id)` | Uninstall by row ID | `DELETE /v1/skills/install/:id` |

## AbortSignal / cancellation

Every method accepts an optional `AbortSignal` as the last argument:

```ts
const controller = new AbortController();
setTimeout(() => controller.abort(), 5000);

const policies = await client.listHotlPolicies(
  { tenant_id: "t1" },
  controller.signal,
);
```

## Custom fetch (testing / Node 16 polyfill)

```ts
import { XiaoguaiClient } from "@xiaoguai/client";

// Use a custom fetch (e.g. node-fetch, undici, or a test mock)
const client = new XiaoguaiClient({
  baseUrl: "http://localhost:8080",
  fetch: myCustomFetch,
});
```

## Bundle format

The package ships both ESM (`dist/index.js`) and CJS (`dist/index.cjs`) with full TypeScript declarations.

## License

MIT
