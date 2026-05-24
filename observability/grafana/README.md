# Xiaoguai Grafana Dashboards

Pre-built Grafana dashboard JSON pack and provisioning configuration for Xiaoguai observability.

## Dashboards

| File | UID | Purpose |
|------|-----|---------|
| `dashboards/xiaoguai-overview.json` | `xiaoguai-overview` | Service health: RPS, p50/p95/p99 latency, error rate, in-flight sessions, DB pool, Redis hit rate |
| `dashboards/xiaoguai-llm.json` | `xiaoguai-llm` | LLM calls/provider, tokens/provider, cost USD/hour/provider, p95 latency/model, error breakdown |
| `dashboards/xiaoguai-scheduler.json` | `xiaoguai-scheduler` | Jobs/sec, duration p50/p95, failure rate, webhook source latency, file-watch events (v1.1.10+), trigger fire rate (F1+) |
| `dashboards/xiaoguai-tenant.json` | `xiaoguai-tenant` | Per-tenant (variable-templated): request rate, error rate, token cost, rate-limit 429s, HOTL budget (F3+), outcome metrics (F4+) |
| `dashboards/xiaoguai-rag.json` | `xiaoguai-rag` | Embeddings/sec, retrieval p95, reranker latency, cache hit-rate proxy |
| `dashboards/xiaoguai-logs.json` | `xiaoguai-logs` | Log volume by level (Loki), error log highlights, full log stream panel |

## Prerequisite Metrics

These dashboards reference metrics produced by the Xiaoguai observability layer (`GET /metrics`
via `metrics-exporter-prometheus`). See the C6 observability runbook for implementation details.

Key metric names:

| Metric | Type | Labels |
|--------|------|--------|
| `xiaoguai_request_total` | Counter | `status`, `status_code`, `route`, `tenant` |
| `xiaoguai_request_duration_seconds` | Histogram | `route`, `model`, `tenant` |
| `xiaoguai_active_sessions` | Gauge | `tenant` |
| `xiaoguai_queue_depth` | Gauge | `worker_pool` |
| `xiaoguai_tokens_total` | Counter | `direction` (input/output), `model`, `provider`, `tenant` |
| `xiaoguai_llm_calls_total` | Counter | `provider`, `model` |
| `xiaoguai_llm_upstream_5xx_total` | Counter | `provider` |
| `xiaoguai_cost_usd_total` | Counter | `provider`, `model`, `tenant` |
| `xiaoguai_db_pool_active_connections` | Gauge | `pool` |
| `xiaoguai_db_pool_max_connections` | Gauge | `pool` |
| `xiaoguai_redis_hit_rate` | Gauge | — |
| `xiaoguai_scheduler_jobs_total` | Counter | `kind`, `status` |
| `xiaoguai_scheduler_job_duration_seconds` | Histogram | `kind` |
| `xiaoguai_scheduler_webhook_requests_total` | Counter | `route_id` |
| `xiaoguai_scheduler_webhook_latency_seconds` | Histogram | `route_id` |
| `xiaoguai_scheduler_file_watch_events_total` | Counter | — (v1.1.10+) |
| `xiaoguai_scheduler_watch_trigger_fires_total` | Counter | — (F1+) |
| `xiaoguai_hotl_budget_consumed_ratio` | Gauge | `tenant` (F3+) |
| `xiaoguai_outcome_success_rate` | Gauge | `tenant` (F4+) |
| `xiaoguai_outcome_goal_completion_rate` | Gauge | `tenant` (F4+) |
| `xiaoguai_rag_embeddings_total` | Counter | `model` |
| `xiaoguai_rag_embedding_duration_seconds` | Histogram | `model` |
| `xiaoguai_rag_retrieval_duration_seconds` | Histogram | `index` |
| `xiaoguai_rag_reranker_duration_seconds` | Histogram | — |
| `xiaoguai_rag_cache_hit_rate` | Gauge | — |
| `xiaoguai_rag_cache_hits_total` | Counter | — |
| `xiaoguai_rag_cache_lookups_total` | Counter | — |
| `xiaoguai_audit_write_failures_total` | Counter | — |

Loki logs require `app="xiaoguai"` and `level` labels (set in the `tracing-subscriber` JSON formatter).

## Loading via Provisioning (Recommended)

Mount these directories into your Grafana container:

```yaml
# docker-compose.yml excerpt
  grafana:
    image: grafana/grafana:10.4.0
    environment:
      - GF_SECURITY_ADMIN_PASSWORD=changeme
      - PROMETHEUS_URL=http://prometheus:9090
      - LOKI_URL=http://loki:3100
    volumes:
      - ./observability/grafana/provisioning:/etc/grafana/provisioning
      - ./observability/grafana/dashboards:/var/lib/grafana/dashboards/xiaoguai
    ports:
      - "3000:3000"
```

Grafana loads datasources and dashboards automatically on startup. No UI clicks required.

## Loading via Import (Manual)

1. Open Grafana UI → Dashboards → Import
2. Upload the JSON file or paste its contents
3. Select the `Prometheus` datasource when prompted
4. Click Import

Repeat for each dashboard file.

## Example docker-compose for Local Testing

```yaml
version: "3.9"

services:
  prometheus:
    image: prom/prometheus:v2.51.0
    command:
      - "--config.file=/etc/prometheus/prometheus.yml"
      - "--storage.tsdb.retention.time=7d"
    volumes:
      - ./deploy/prometheus.yml:/etc/prometheus/prometheus.yml:ro
    ports:
      - "9090:9090"

  loki:
    image: grafana/loki:2.9.6
    command: -config.file=/etc/loki/local-config.yaml
    ports:
      - "3100:3100"

  grafana:
    image: grafana/grafana:10.4.0
    depends_on: [prometheus, loki]
    environment:
      - GF_SECURITY_ADMIN_PASSWORD=changeme
      - PROMETHEUS_URL=http://prometheus:9090
      - LOKI_URL=http://loki:3100
    volumes:
      - ./observability/grafana/provisioning:/etc/grafana/provisioning
      - ./observability/grafana/dashboards:/var/lib/grafana/dashboards/xiaoguai
    ports:
      - "3000:3000"
```

Start with:

```bash
docker compose -f docker-compose.yml up prometheus loki grafana
```

Open `http://localhost:3000` (admin / changeme). All five dashboards appear under
Dashboards → Browse → Xiaoguai.

## Deferred

- **Tempo traces dashboard** — distributed trace panels (service map, span waterfall) are
  deferred until the OTLP gRPC trace export (ADR-0013 Tier 1) is wired into the Grafana
  Tempo datasource. The panels require `DS_TEMPO` and a running Tempo instance; placeholder
  UID reserved as `xiaoguai-traces`.
