# Observability Runbook

**Crate:** `xiaoguai-observability` (v1.2.11)  
**Feature flag:** `observability` on `xiaoguai-core`

---

## Overview

When the `observability` Cargo feature is enabled, `xiaoguai-core` exposes:

| Signal | Transport | Endpoint / destination |
|---|---|---|
| Prometheus metrics | HTTP text format | `GET /metrics` on the main API port |
| Distributed traces | OTLP/gRPC | `OTEL_EXPORTER_OTLP_ENDPOINT` (default `http://localhost:4317`) |

---

## Enabling the feature

Build or run `xiaoguai-core` with the feature flag:

```bash
# development
cargo run -p xiaoguai-core --features observability

# release binary
cargo build --release -p xiaoguai-core --features observability
```

The Docker image and systemd unit must be rebuilt with `--features observability`
to include the telemetry layers.

---

## Environment variables

| Variable | Default | Description |
|---|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | OTLP/gRPC collector endpoint |
| `OTEL_SERVICE_NAME` | `xiaoguai` | Overrides the service name on every span |
| `RUST_LOG` | `info,sqlx=warn` | Controls which spans/logs are emitted |

---

## Connecting to a real Prometheus

### Prometheus scrape config

Add to `prometheus.yml`:

```yaml
scrape_configs:
  - job_name: "xiaoguai"
    scrape_interval: 15s
    static_configs:
      - targets: ["<xiaoguai-host>:7600"]
    # If TLS termination is upstream, set scheme: https and tls_config.
    metrics_path: /metrics
```

Verify the endpoint manually:

```bash
curl -s http://localhost:7600/metrics | head -40
```

You should see lines such as:

```
# HELP xiaoguai_http_request_duration_seconds HTTP request latency in seconds
# TYPE xiaoguai_http_request_duration_seconds histogram
xiaoguai_http_request_duration_seconds_bucket{method="GET",path="/v1/sessions",status="200",le="0.001"} 0
...
xiaoguai_http_request_duration_seconds_count{method="GET",path="/v1/sessions",status="200"} 42
```

### Sample PromQL queries

```promql
# P99 HTTP latency across all routes (5m window)
histogram_quantile(0.99,
  sum by (le, path) (
    rate(xiaoguai_http_request_duration_seconds_bucket[5m])
  )
)

# LLM call rate by provider
rate(xiaoguai_llm_call_duration_seconds_count[1m])

# Scheduler tick P50
histogram_quantile(0.5,
  rate(xiaoguai_scheduler_tick_duration_seconds_bucket[5m])
)
```

---

## Connecting to an OpenTelemetry Collector

### Minimal OTel Collector pipeline

`otel-collector-config.yaml`:

```yaml
receivers:
  otlp:
    protocols:
      grpc:
        endpoint: "0.0.0.0:4317"

processors:
  batch:
    timeout: 5s
    send_batch_size: 512

exporters:
  # Send traces to Jaeger
  jaeger:
    endpoint: "jaeger:14250"
    tls:
      insecure: true
  # Also log spans locally for debugging
  logging:
    verbosity: detailed

service:
  pipelines:
    traces:
      receivers: [otlp]
      processors: [batch]
      exporters: [jaeger, logging]
```

Run with Docker Compose alongside Xiaoguai:

```yaml
# docker-compose.override.yml
services:
  otel-collector:
    image: otel/opentelemetry-collector-contrib:0.106.1
    command: ["--config=/etc/otel-collector-config.yaml"]
    volumes:
      - ./otel-collector-config.yaml:/etc/otel-collector-config.yaml
    ports:
      - "4317:4317"   # OTLP gRPC
      - "55679:55679" # zPages debug UI

  jaeger:
    image: jaegertracing/all-in-one:1.60
    ports:
      - "16686:16686" # Jaeger UI
      - "14250:14250" # gRPC receiver
```

Point Xiaoguai at the collector:

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector:4317
```

---

## Exported metrics reference

| Metric | Type | Labels | Description |
|---|---|---|---|
| `xiaoguai_http_request_duration_seconds` | Histogram | `method`, `path`, `status` | HTTP request latency |
| `xiaoguai_llm_call_duration_seconds` | Histogram | `provider`, `model` | LLM call latency |
| `xiaoguai_scheduler_tick_duration_seconds` | Histogram | — | Scheduler tick latency |
| `process_*` | Gauge/Counter | — | Linux process metrics (CPU, memory, FDs) |

Histograms use exponential buckets: 1 ms, 2 ms, 4 ms … up to ~65 s.

---

## Instrumenting application code

Use the macros from `xiaoguai-observability` to emit both a span and a
histogram observation in one call:

```rust
use xiaoguai_observability::{instrument_llm_call, instrument_http_request};

// Inside an async fn:
let response = instrument_llm_call!("ollama", "qwen2.5", async {
    backend.chat(&messages).await
});

let resp = instrument_http_request!("GET", "/v1/sessions", "200", async {
    handler(req).await
});
```

The macros are no-ops when `init_prometheus` was not called.

---

## Graceful shutdown

The OTLP batch exporter holds an in-memory queue. Call the global
shutdown function during graceful exit to flush buffered spans:

```rust
// In main.rs shutdown path:
xiaoguai_observability::shutdown();
```

This is already wired when the `observability` feature is active.

---

## Troubleshooting

**`/metrics` returns 404**  
The binary was built without `--features observability`. Rebuild with the flag.

**No traces in Jaeger**  
Check `OTEL_EXPORTER_OTLP_ENDPOINT` — the default `localhost:4317` assumes
the collector runs on the same host. Verify with:
```bash
grpcurl -plaintext localhost:4317 list
```

**`init observability` error on startup**  
The tracing subscriber was already initialised (e.g. by a test harness or
another crate calling `tracing_subscriber::fmt().init()`). Ensure `mount`
is called exactly once before any span is created.

**`process_*` metrics missing on macOS**  
The process collector is Linux-only (`procfs` dependency). This is
expected — the Xiaoguai-specific histograms still work on all platforms.

**Large cardinality on `path` label**  
The `path` label uses the raw URI path. For parameterised routes such as
`/v1/sessions/some-uuid`, instrument at the route handler level and pass
the route template (e.g. `/v1/sessions/:id`) rather than the resolved URI.
