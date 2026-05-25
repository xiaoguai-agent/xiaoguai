# xiaoguai-client — Java SDK

Java 21 client for the xiaoguai REST API (wave-3 endpoints).

**Maven coordinates**: `io.github.xiaoguai-agent:xiaoguai-client:0.1.0`

## Requirements

- Java 21+
- Maven 3.9+

## Quick start

```java
try (XiaoguaiClient client = XiaoguaiClient.builder()
        .baseUrl("http://localhost:8080")
        .bearerToken("my-token")
        .build()) {

    // HotL — boundary policy management
    HotlPolicy policy = client.hotl().createHotlPolicy(
            "tenant-uuid", "llm_call", 3600, 100, 5.00, "#ops-alerts");
    List<HotlPolicy> policies = client.hotl().listHotlPolicies("tenant-uuid");

    // Outcomes — ROI telemetry
    client.outcomes().recordOutcome("tenant-uuid", "sales-bot", "revenue_usd", 1200.0);
    OutcomeSummary summary = client.outcomes().outcomesSummary("tenant-uuid", "30d");

    // Skills — pack marketplace
    List<SkillCatalogEntry> catalog = client.skills().listSkillCatalog();
    InstalledSkillPack pack = client.skills().installSkill("tenant-uuid", "rag-legal");
}
```

## API surface

### HotlApi (`client.hotl()`)

| Method | HTTP | Description |
|--------|------|-------------|
| `createHotlPolicy(...)` | POST /v1/hotl/policies | Create a boundary policy |
| `listHotlPolicies(tenantId)` | GET /v1/hotl/policies | List policies for a tenant |
| `getHotlPolicy(policyId)` | GET /v1/hotl/policies/:id | Get a single policy |
| `updateHotlPolicy(...)` | POST /v1/hotl/policies/:id | Update a policy |
| `deleteHotlPolicy(policyId)` | DELETE /v1/hotl/policies/:id | Delete a policy |
| `checkHotl(tenantId, scope, amount)` | POST /v1/hotl/check | Pre-flight budget check |

### OutcomesApi (`client.outcomes()`)

| Method | HTTP | Description |
|--------|------|-------------|
| `recordOutcome(...)` | POST /v1/outcomes | Record an outcome attribution |
| `listOutcomes(tenantId)` | GET /v1/outcomes | List raw outcome records |
| `outcomesSummary(tenantId)` | GET /v1/outcomes/summary | Aggregated ROI summary |
| `outcomesTimeseries(tenantId)` | GET /v1/outcomes/timeseries | Daily time-series |

### SkillsApi (`client.skills()`)

| Method | HTTP | Description |
|--------|------|-------------|
| `listInstalledSkills(tenantId)` | GET /v1/skills/installed | List installed packs |
| `listSkillCatalog()` | GET /v1/skills/catalog | List catalog entries |
| `installSkill(tenantId, slug)` | POST /v1/skills/install | Install a pack |
| `uninstallSkill(installId)` | DELETE /v1/skills/install/:id | Uninstall a pack |

## Error handling

All error classes extend `XiaoguaiException` (unchecked):

| Class | HTTP status |
|-------|-------------|
| `AuthException` | 401, 403 |
| `NotFoundException` | 404 |
| `ConflictException` | 409 |
| `RateLimitException` | 429 |
| `ServerException` | 5xx |
| `HttpException` | other non-2xx |

## Build & test

```bash
cd sdk/java
mvn clean verify
```

## Design notes

- **Records** for all immutable model types (Java 21)
- **Sealed `HttpException`** hierarchy for type-safe error handling
- **`java.net.http.HttpClient`** with virtual-thread executor — no Netty or OkHttp dependency
- **Jackson Databind** for JSON — idiomatic, well-maintained
- **WireMock + JUnit 5 + AssertJ** for tests — no mocking frameworks needed
- Thread-safe; one instance per application is recommended
