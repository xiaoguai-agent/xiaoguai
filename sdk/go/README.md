# xiaoguai-go-client

Go client for the [Xiaoguai](https://github.com/xiaoguai-agent/xiaoguai) agent-platform REST API (wave-3 endpoints).

## Install

```sh
go get github.com/xiaoguai-agent/xiaoguai-go-client
```

Requires Go 1.22+. No external HTTP library — stdlib `net/http` only.

## Quick Start

```go
import xiaoguai "github.com/xiaoguai-agent/xiaoguai-go-client"

client, err := xiaoguai.NewClient("http://localhost:8080",
    xiaoguai.WithToken("my-bearer-token"),
)

// HotL policies
policies, err := client.ListHotlPolicies(ctx, "my-tenant-id")

maxCount := 100
policy, err := client.CreateHotlPolicy(ctx, xiaoguai.CreateHotlPolicyRequest{
    TenantID:      "my-tenant-id",
    Scope:         "llm_call",
    WindowSeconds: 3600,
    MaxCount:      &maxCount,
})
err = client.DeleteHotlPolicy(ctx, policy.ID)

// Outcomes
ok, err := client.RecordOutcome(ctx, xiaoguai.RecordOutcomeRequest{
    TenantID:  "my-tenant-id",
    AgentName: "sales-bot",
    Kind:      "revenue_usd",
    Value:     1200.0,
})
summary, err := client.OutcomesSummary(ctx, "my-tenant-id", xiaoguai.WithRange("30d"))
ts, err := client.OutcomesTimeseries(ctx, "my-tenant-id", xiaoguai.WithRange("7d"))

// Skills
catalog, err := client.ListSkillCatalog(ctx)
installed, err := client.ListInstalledSkills(ctx, "my-tenant-id")
pack, err := client.InstallSkill(ctx, xiaoguai.InstallSkillRequest{
    TenantID: "my-tenant-id",
    PackSlug: "rag-legal",
})
deleted, err := client.UninstallSkill(ctx, pack.ID)
```

## API Surface

| Method | HTTP | Description |
|---|---|---|
| `ListHotlPolicies` | `GET /v1/hotl/policies` | List boundary policies for a tenant |
| `CreateHotlPolicy` | `POST /v1/hotl/policies` | Create a boundary policy |
| `DeleteHotlPolicy` | `DELETE /v1/hotl/policies/:id` | Delete a policy |
| `RecordOutcome` | `POST /v1/outcomes` | Record an outcome attribution |
| `OutcomesSummary` | `GET /v1/outcomes/summary` | Aggregated ROI summary |
| `OutcomesTimeseries` | `GET /v1/outcomes/timeseries` | Daily time-series breakdown |
| `ListSkillCatalog` | `GET /v1/skills/catalog` | List available skill packs |
| `ListInstalledSkills` | `GET /v1/skills/installed` | List installed packs for a tenant |
| `InstallSkill` | `POST /v1/skills/install` | Install a skill pack |
| `UninstallSkill` | `DELETE /v1/skills/install/:id` | Uninstall a skill pack |

## Error Handling

```go
import "errors"

_, err := client.InstallSkill(ctx, req)
var conflict *xiaoguai.ConflictError
if errors.As(err, &conflict) {
    // pack already installed
}
```

Error types: `HTTPError` (base), `AuthError` (401), `NotFoundError` (404),
`ValidationError` (400/422), `ConflictError` (409), `RateLimitError` (429),
`ServerError` (5xx).

## Retries

5xx responses are retried up to 3 times with exponential back-off (100 ms base, 2 s cap).
Context cancellation is respected at every retry boundary.

## Options

```go
xiaoguai.WithToken("bearer-token")           // Authorization header
xiaoguai.WithHTTPClient(customClient)        // inject custom http.Client
xiaoguai.WithTimeout(10 * time.Second)       // override 30 s default
xiaoguai.WithLogger(myLogger)                // verbose logging (Logger interface)
```

## Run the Example

```sh
XIAOGUAI_BASE_URL=http://localhost:8080 XIAOGUAI_TOKEN=tok go run ./cmd/example
```

## License

Apache License 2.0 — see [LICENSE](LICENSE).
