# Xiaoguai API Client SDK — Comparison & Selection Guide

> **API version**: 1.2.28 (wave-3)  
> **Official SDKs**: Python · TypeScript · Go · Java  
> **Other languages**: generate from `docs/api/openapi.yaml` — see [OpenAPI Generator Quickstart](#10-openapi-generator-quickstart)

---

## 1. When to Use Which SDK

| Use case | Recommended SDK | Why |
|---|---|---|
| Data engineering, notebooks, AI/ML pipelines | **Python** | `pip install xiaoguai[client]`; pandas / langchain friendly; sync-first avoids asyncio boilerplate in Jupyter |
| Web apps, Node.js services, edge functions (Cloudflare Workers) | **TypeScript** | ESM + CJS dual build; zero runtime deps; tree-shakeable; native `fetch` with injected mock support |
| High-throughput backends, sidecars, infra tooling | **Go** | stdlib `net/http`; context-first cancellation; built-in exponential-backoff retry; goroutine-safe |
| Enterprise services, JVM-based agents, Spring Boot, Android | **Java** | Java 21+ `HttpClient` with virtual-thread executor; Jackson polymorphism; `AutoCloseable` lifecycle |
| Ruby, Rust, .NET, Kotlin, Swift, PHP, or any other language | **OpenAPI generator** | Generate from `docs/api/openapi.yaml`; see [Section 10](#10-openapi-generator-quickstart) |

---

## 2. Feature Parity Matrix

Endpoints defined in the wave-3 OpenAPI spec (`docs/api/openapi.yaml`) across all four official SDKs.

| Endpoint | Python | TypeScript | Go | Java |
|---|:---:|:---:|:---:|:---:|
| `GET /v1/hotl/policies` | ✅ | ✅ | ✅ | ✅ |
| `POST /v1/hotl/policies` | ✅ | ✅ | ✅ | ✅ |
| `GET /v1/hotl/policies/:id` | ❌ server gap | ❌ server gap | ❌ server gap | ✅ |
| `PUT /v1/hotl/policies/:id` | ❌ server gap | ❌ server gap | ❌ server gap | ✅ |
| `DELETE /v1/hotl/policies/:id` | ✅ | ✅ | ✅ | ✅ |
| `POST /v1/hotl/check` | 🚧 placeholder | 🚧 placeholder | 🚧 placeholder | ✅ |
| `POST /v1/outcomes` | ✅ | ✅ | ✅ | ✅ |
| `GET /v1/outcomes/summary` | ✅ | ✅ | ✅ | ✅ |
| `GET /v1/outcomes/timeseries` | ✅ | ✅ | ✅ | ✅ |
| `GET /v1/outcomes` (raw list) | 🚧 placeholder | 🚧 placeholder | 🚧 placeholder | 🚧 placeholder |
| `GET /v1/skills/catalog` | ✅ | ✅ | ✅ | ✅ |
| `GET /v1/skills/installed` | ✅ | ✅ | ✅ | ✅ |
| `POST /v1/skills/install` | ✅ | ✅ | ✅ | ✅ |
| `DELETE /v1/skills/install/:id` | ✅ | ✅ | ✅ | ✅ |

**Legend**: ✅ implemented · 🚧 stub / placeholder (server endpoint not yet wired) · ❌ method raises `NotImplementedError` / `UnsupportedOperationException`

> **Java note**: Java is the only SDK that implements `getHotlPolicy`, `updateHotlPolicy`, and `checkHotl` as callable methods (the server endpoints exist in the OpenAPI spec; they are stubs in other SDKs pending server wiring).

---

## 3. Lang-Specific Gotchas

| SDK | Gotcha | Mitigation |
|---|---|---|
| **Python** | SDK is **synchronous** (blocking `httpx.Client`). Calling it directly inside an `asyncio` event loop blocks the thread. | Wrap with `asyncio.get_event_loop().run_in_executor(None, fn)` or run the client call in a `ThreadPoolExecutor`. Never `await` a sync function directly. |
| **Python** | `httpx` is an optional dependency — `ImportError` at import time if missing. | Install via `pip install 'xiaoguai[client]'` (or `pip install httpx`). |
| **TypeScript** | **Browser CORS**: the API server must send `Access-Control-Allow-Origin` headers or preflight `OPTIONS` requests will fail with a network error (not a 401). | In browser contexts, proxy requests through your own backend. For Node.js / edge runtimes, CORS is irrelevant. |
| **TypeScript** | Node.js < 18 has no global `fetch`. | Pass a `fetch` implementation (e.g. `node-fetch`) in `XiaoguaiClientConfig.fetch`. |
| **Go** | **Context cancellation** is propagated to the HTTP layer via `context.Context`; each retry checks `ctx.Err()` before sleeping. Forgetting to pass a context allows a goroutine to hang indefinitely. | Always pass a context with a deadline: `ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second); defer cancel()`. |
| **Go** | The built-in retry middleware retries **5xx only** (not 429). Adding rate-limit retry requires wrapping `Client.WithHTTPClient` with a custom `http.RoundTripper`. | See the [Error Handling](#6-error-handling) section for a 429 pattern. |
| **Java** | Requires **JDK 21+** for virtual threads (`Executors.newVirtualThreadPerTaskExecutor()`). Running on JDK 17/11 causes a `NoSuchMethodError` at startup. | Enforce `<java.version>21</java.version>` in `pom.xml`. The SDK `pom.xml` already does this. |
| **Java** | **Jackson polymorphism on `HotlVerdict.Kind`**: the enum uses `@JsonValue` + `@JsonCreator` for lowercase wire values (`"allow"`, `"escalate"`, `"deny"`). Custom `ObjectMapper` configurations that disable `MapperFeature.ALLOW_EXPLICIT_PROPERTY_RENAMING` will break deserialization. | Use the default `ObjectMapper` supplied by `JsonCodec`, or register `xiaoguai` as a Jackson module. |

---

## 4. Auth Pattern

All four SDKs use `Authorization: Bearer <token>`. The token is set once at client construction and sent on every request.

**Python**
```python
from xiaoguai.client import XiaoguaiClient

with XiaoguaiClient("https://api.example.com", token="xg-tok-...") as client:
    policies = client.list_hotl_policies(tenant_id="11111111-...")
```

**TypeScript**
```typescript
import { XiaoguaiClient } from "@xiaoguai/client";

const client = new XiaoguaiClient({
  baseUrl: "https://api.example.com",
  token: "xg-tok-...",
});
const policies = await client.listHotlPolicies({ tenant_id: "11111111-..." });
```

**Go**
```go
import "github.com/xiaoguaiagent/xiaoguai-go"

client, err := xiaoguai.NewClient(
    "https://api.example.com",
    xiaoguai.WithToken("xg-tok-..."),
)
if err != nil { /* handle */ }
policies, err := client.ListHotlPolicies(ctx, "11111111-...")
```

**Java**
```java
import io.github.xiaoguaiagent.client.XiaoguaiClient;

XiaoguaiClient client = XiaoguaiClient.builder()
    .baseUrl("https://api.example.com")
    .token("xg-tok-...")
    .build();
List<HotlPolicy> policies = client.hotl().listHotlPolicies("11111111-...");
```

> **Token management**: store tokens in environment variables (`XIAOGUAI_TOKEN`), not in source code. The API returns `401` with a `WWW-Authenticate: Bearer` challenge (RFC 6750 §3) when the token is missing or expired.

---

## 5. Pagination and Iteration

The server returns aggregated data for outcomes (summary and timeseries) rather than raw paginated lists. `GET /v1/outcomes` raw-list is not yet exposed; use the aggregate endpoints. For skill catalogs and installed packs, all results are returned in a single response (no pagination cursor yet).

When the raw-list endpoint ships, the following idioms will apply for iterating over > 1 000 `listOutcomes` records:

**Python** — iterator wrapping page calls
```python
import itertools

def iter_outcomes(client, tenant_id, page_size=100):
    offset = 0
    while True:
        page = client.list_outcomes(filter={"tenant_id": tenant_id,
                                            "limit": page_size,
                                            "offset": offset})
        if not page:
            break
        yield from page
        if len(page) < page_size:
            break
        offset += page_size

for outcome in itertools.islice(iter_outcomes(client, "tenant-1"), 5000):
    process(outcome)
```

**TypeScript** — async generator
```typescript
async function* iterOutcomes(
  client: XiaoguaiClient,
  tenantId: string,
  pageSize = 100,
): AsyncGenerator<unknown> {
  let offset = 0;
  while (true) {
    const page = await client.listOutcomes({ tenant_id: tenantId, limit: pageSize, offset });
    for (const item of page) yield item;
    if (page.length < pageSize) break;
    offset += pageSize;
  }
}

for await (const outcome of iterOutcomes(client, "tenant-1")) {
  process(outcome);
}
```

**Go** — callback-based iterator
```go
func iterOutcomes(ctx context.Context, c *xiaoguai.Client, tenantID string, fn func(o xiaoguai.OutcomeRecord) error) error {
    const pageSize = 100
    offset := 0
    for {
        page, err := c.ListOutcomes(ctx, tenantID, xiaoguai.WithLimit(pageSize), xiaoguai.WithOffset(offset))
        if err != nil { return err }
        for _, o := range page {
            if err := fn(o); err != nil { return err }
        }
        if len(page) < pageSize { return nil }
        offset += pageSize
    }
}
```

**Java** — `Stream` via `Spliterator`
```java
Stream<OutcomeRecord> streamOutcomes(XiaoguaiClient client, String tenantId) {
    return StreamSupport.stream(new OutcomeSpliterator(client, tenantId, 100), false);
}
// OutcomeSpliterator implements Spliterator<OutcomeRecord> with tryAdvance
// calling client.outcomes().listOutcomes(tenantId, limit, offset)
```

---

## 6. Error Handling

### Error hierarchy per SDK

| HTTP Status | Python | TypeScript | Go | Java |
|---|---|---|---|---|
| 400 / 422 | `XiaoguaiValidationError` | `ValidationError` | `*ValidationError` | `ValidationException` |
| 401 | `XiaoguaiHTTPError` (status=401) | `AuthError` | `*AuthError` | `AuthException` |
| 403 | `XiaoguaiHTTPError` (status=403) | `ForbiddenError` | `*HTTPError` (status=403) | `HttpException` |
| 404 | `XiaoguaiNotFoundError` | `NotFoundError` | `*NotFoundError` | `NotFoundException` |
| 409 | `XiaoguaiConflictError` | `ConflictError` | `*ConflictError` | `ConflictException` |
| 429 | `XiaoguaiHTTPError` (status=429) | `RateLimitError` (+ `retryAfter`) | `*RateLimitError` | `RateLimitException` |
| 5xx | `XiaoguaiHTTPError` | `ServerError` | `*ServerError` (after retries) | `ServerException` |

All base classes: `XiaoguaiHTTPError` · `XiaoguaiError` · `HTTPError` · `XiaoguaiException`.

### Canonical "handle 429 with retry" snippet

**Python**
```python
import time
from xiaoguai.client import XiaoguaiClient, XiaoguaiHTTPError

def with_retry(fn, max_attempts=3):
    for attempt in range(max_attempts):
        try:
            return fn()
        except XiaoguaiHTTPError as e:
            if e.status_code == 429 and attempt < max_attempts - 1:
                wait = int(e.body.get("retry_after", 2 ** attempt))
                time.sleep(wait)
            else:
                raise

result = with_retry(lambda: client.list_hotl_policies(tenant_id="..."))
```

**TypeScript**
```typescript
import { RateLimitError } from "@xiaoguai/client";

async function withRetry<T>(fn: () => Promise<T>, maxAttempts = 3): Promise<T> {
  for (let attempt = 0; attempt < maxAttempts; attempt++) {
    try {
      return await fn();
    } catch (e) {
      if (e instanceof RateLimitError && attempt < maxAttempts - 1) {
        const waitMs = (e.retryAfter ?? 2 ** attempt) * 1000;
        await new Promise((r) => setTimeout(r, waitMs));
      } else {
        throw e;
      }
    }
  }
  throw new Error("unreachable");
}
```

**Go**
```go
import (
    "errors"
    "time"
    "github.com/xiaoguaiagent/xiaoguai-go"
)

func withRetry(ctx context.Context, fn func() error, maxAttempts int) error {
    for attempt := 0; attempt < maxAttempts; attempt++ {
        err := fn()
        if err == nil {
            return nil
        }
        var rl *xiaoguai.RateLimitError
        if errors.As(err, &rl) && attempt < maxAttempts-1 {
            select {
            case <-ctx.Done():
                return ctx.Err()
            case <-time.After(time.Duration(1<<attempt) * time.Second):
            }
            continue
        }
        return err
    }
    return nil
}
```

**Java**
```java
import io.github.xiaoguaiagent.client.error.RateLimitException;
import java.util.concurrent.Callable;

public static <T> T withRetry(Callable<T> fn, int maxAttempts) throws Exception {
    for (int attempt = 0; attempt < maxAttempts; attempt++) {
        try {
            return fn.call();
        } catch (RateLimitException e) {
            if (attempt >= maxAttempts - 1) throw e;
            long waitMs = e.getRetryAfterSeconds().map(s -> s * 1000L)
                           .orElse((long) Math.pow(2, attempt) * 1000L);
            Thread.sleep(waitMs);
        }
    }
    throw new IllegalStateException("unreachable");
}
```

---

## 7. Testing Pattern

Each SDK supports injecting a mock HTTP layer without starting a real server.

**Python — `httpx.MockTransport`**
```python
import json
import httpx
import pytest
from xiaoguai.client import XiaoguaiClient, HotlPolicy

def make_mock_transport(status: int, body: dict) -> httpx.MockTransport:
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(status, json=body)
    return httpx.MockTransport(handler)

def test_list_policies():
    payload = [{"id": "abc", "tenant_id": "t1", "scope": "llm_call",
                "window_seconds": 3600, "max_count": 100}]
    transport = make_mock_transport(200, payload)
    with XiaoguaiClient("http://mock", token="test", transport=transport) as c:
        policies = c.list_hotl_policies(tenant_id="t1")
    assert len(policies) == 1
    assert isinstance(policies[0], HotlPolicy)
```

**TypeScript — custom `fetch` mock (vitest)**
```typescript
import { vi, expect, it } from "vitest";
import { XiaoguaiClient } from "@xiaoguai/client";

it("listHotlPolicies returns parsed policies", async () => {
  const payload = [{ id: "abc", tenant_id: "t1", scope: "llm_call", window_seconds: 3600 }];
  const mockFetch = vi.fn().mockResolvedValue(
    new Response(JSON.stringify(payload), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    }),
  );
  const client = new XiaoguaiClient({ baseUrl: "http://mock", fetch: mockFetch });
  const policies = await client.listHotlPolicies({ tenant_id: "t1" });
  expect(policies).toHaveLength(1);
});
```

**Go — `httptest.Server`**
```go
import (
    "encoding/json"
    "net/http"
    "net/http/httptest"
    "testing"
    "github.com/xiaoguaiagent/xiaoguai-go"
)

func TestListHotlPolicies(t *testing.T) {
    srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
        w.Header().Set("Content-Type", "application/json")
        json.NewEncoder(w).Encode([]map[string]any{
            {"id": "abc", "tenant_id": "t1", "scope": "llm_call", "window_seconds": 3600},
        })
    }))
    defer srv.Close()

    c, _ := xiaoguai.NewClient(srv.URL, xiaoguai.WithToken("test"))
    policies, err := c.ListHotlPolicies(t.Context(), "t1")
    if err != nil || len(policies) != 1 {
        t.Fatalf("unexpected: err=%v len=%d", err, len(policies))
    }
}
```

**Java — WireMock**
```java
import com.github.tomakehurst.wiremock.junit5.WireMockTest;
import static com.github.tomakehurst.wiremock.client.WireMock.*;
import org.junit.jupiter.api.Test;

@WireMockTest(httpPort = 8099)
class HotlApiTest {

    @Test
    void listPoliciesReturnsItems() {
        stubFor(get(urlPathEqualTo("/v1/hotl/policies"))
            .withQueryParam("tenant_id", equalTo("t1"))
            .willReturn(okJson("""
                [{"id":"abc","tenant_id":"t1","scope":"llm_call","window_seconds":3600}]
            """)));

        XiaoguaiClient client = XiaoguaiClient.builder()
            .baseUrl("http://localhost:8099").token("test").build();
        var policies = client.hotl().listHotlPolicies("t1");
        assertEquals(1, policies.size());
    }
}
```

---

## 8. Versioning and Compatibility

The Xiaoguai API and all official SDKs follow **semantic versioning** (`MAJOR.MINOR.PATCH`).

| Version component | Guarantee |
|---|---|
| **PATCH** bump | Backwards-compatible bug fixes. Safe to accept automatically. |
| **MINOR** bump | New endpoints or fields added. Existing behaviour unchanged. |
| **MAJOR** bump | Breaking changes. Migration guide published alongside release. |

**Deprecation window**: deprecated endpoints and SDK methods are supported for a minimum of **12 months** after the deprecation notice. Deprecated items emit a warning header (`Deprecation: true`) from the server and a `DeprecationWarning` / console warning from the SDK.

**Pinning recommendations**:

```
# Python
xiaoguai[client]>=1.2,<2.0

# npm (package.json)
"@xiaoguai/client": "^1.2.0"

# Go (go.mod)
require github.com/xiaoguaiagent/xiaoguai-go v1.2.28

# Maven (pom.xml)
<version>[1.2,2.0)</version>
```

---

## 9. Migration from Raw HTTP

Equivalent operations: create a HOTL policy with `curl` vs. each SDK.

**curl**
```bash
curl -s -X POST https://api.example.com/v1/hotl/policies \
  -H "Authorization: Bearer xg-tok-..." \
  -H "Content-Type: application/json" \
  -d '{"tenant_id":"t1","scope":"llm_call","window_seconds":3600,"max_count":100}'
```

**Python**
```python
policy = client.create_hotl_policy(
    tenant_id="t1", scope="llm_call", window_seconds=3600, max_count=100
)
```

**TypeScript**
```typescript
const policy = await client.createHotlPolicy({
  tenant_id: "t1", scope: "llm_call", window_seconds: 3600, max_count: 100,
});
```

**Go**
```go
policy, err := client.CreateHotlPolicy(ctx, xiaoguai.CreateHotlPolicyRequest{
    TenantID: "t1", Scope: "llm_call", WindowSeconds: 3600, MaxCount: ptr(100),
})
```

**Java**
```java
HotlPolicy policy = client.hotl().createHotlPolicy("t1", "llm_call", 3600, 100, null, null);
```

The SDK handles: `Authorization` header injection, JSON serialisation, response deserialisation into typed models, error classification into the exception hierarchy, and (Go) exponential-backoff retry on 5xx.

---

## 10. OpenAPI Generator Quickstart

For languages not covered by an official SDK, generate a client from `docs/api/openapi.yaml` (branch `docs/openapi-wave3`).

**Get the spec**
```bash
git clone --depth 1 --branch docs/openapi-wave3 \
  https://github.com/xiaoguaiagent/xiaoguai.git xiaoguai-spec
SPEC=./xiaoguai-spec/docs/api/openapi.yaml
```

### Recommended generators

| Target language | Recommended tool | Install |
|---|---|---|
| TypeScript (alternative to official SDK) | `openapi-typescript-codegen` | `npm i -g openapi-typescript-codegen` |
| Java, Kotlin, C#, Ruby, PHP, Swift, Rust, Go, Python | `openapi-generator-cli` | `npm i -g @openapitools/openapi-generator-cli` |

### TypeScript (via `openapi-typescript-codegen`)
```bash
openapi-ts \
  --input "$SPEC" \
  --output ./src/xiaoguai-api \
  --client fetch \
  --useOptions \
  --exportSchemas true
```

### Java (via `openapi-generator-cli`)
```bash
openapi-generator-cli generate \
  -i "$SPEC" \
  -g java \
  --library native \
  -o ./xiaoguai-java-generated \
  --additional-properties=useJakartaEe=true,java8=true,openApiNullable=false
```

### Ruby
```bash
openapi-generator-cli generate -i "$SPEC" -g ruby -o ./xiaoguai-ruby
```

### Rust
```bash
openapi-generator-cli generate -i "$SPEC" -g rust -o ./xiaoguai-rust \
  --additional-properties=packageName=xiaoguai-client
```

### .NET / C#
```bash
openapi-generator-cli generate -i "$SPEC" -g csharp -o ./xiaoguai-dotnet \
  --additional-properties=packageName=Xiaoguai.Client,targetFramework=net8.0
```

### Kotlin
```bash
openapi-generator-cli generate -i "$SPEC" -g kotlin -o ./xiaoguai-kotlin \
  --additional-properties=library=jvm-ktor
```

**Authentication note**: generated clients may not inject the `Authorization` header automatically. Set it via the generator's `ApiClient.setApiKey(token)` pattern or pass `--additional-properties=apiKeyPrefix=Bearer,apiKey=<token>`.

> **Tested generator versions**: `openapi-typescript-codegen` v0.25+ · `openapi-generator-cli` v7.5+  
> Earlier versions may not handle OpenAPI 3.1 `nullable` correctly.

---

## Roadmap

> **Planned official SDKs**: Ruby, .NET (NuGet), and a Rust crate (`xiaoguai-client`) are on the roadmap for a future release. Community contributions are welcome — open an issue or PR against the [xiaoguai repository](https://github.com/xiaoguaiagent/xiaoguai) with your generated or hand-written client.

---

*Last updated: 2026-05-25 — wave-3 (API v1.2.28)*
