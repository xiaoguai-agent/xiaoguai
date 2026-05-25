# OpenTelemetry Collector — Advanced Config

Production-grade companion to the basic collector config
(`deploy/otel-collector-config.yaml`). Adds tail sampling, PII
redaction, Prometheus exemplars, and S3 long-term archive.

## When to use this vs the basic config

| Dimension | Basic config | Advanced config |
|-----------|-------------|-----------------|
| Purpose | Dev / CI / staging smoke test | Production |
| Sampling | None (all spans exported) | Tail sampling (1% normal, 100% errors/slow/escalate) |
| PII protection | None | 10 attribute patterns dropped or hashed |
| Exemplars | No | Yes — trace_id injected per histogram bucket |
| Long-term archive | No | S3 with date-rolling prefix |
| External deps | None | Tempo, S3, Loki (optional) |

Switch by updating the OTel SDK endpoint in `config.example.yaml` to
point to a collector running the advanced config, or run both collectors
in parallel (different ports / services) during the cutover period.

## Environment variables

All operator knobs are injected via environment variables — no YAML
edit required after initial setup.

| Variable | Required | Example | Purpose |
|----------|----------|---------|---------|
| `OTEL_CLUSTER_NAME` | yes | `prod-us-east-1` | Cluster label stamped on all telemetry |
| `OTEL_AWS_REGION` | yes | `us-east-1` | S3 upload region + cloud.region attribute |
| `OTEL_ENVIRONMENT` | yes | `production` | `deployment.environment` attribute |
| `OTEL_TEMPO_ENDPOINT` | yes | `tempo:4317` | Grafana Tempo gRPC endpoint |
| `OTEL_TEMPO_CA_FILE` | yes* | `/certs/tempo-ca.pem` | TLS CA for Tempo; omit if `insecure: true` |
| `OTEL_S3_BUCKET` | yes | `xiaoguai-otel-archive` | S3 bucket for archived traces + logs |
| `OTEL_LOKI_ENDPOINT` | yes | `loki:3100/otlp` | Loki OTLP endpoint for log export |
| `OTEL_LOKI_INSECURE` | no | `false` | Set `true` for self-signed Loki certs |
| `OTEL_WATCHLIST_TENANT_1..3` | no | `tenant-abc` | Tenant IDs sampled at 100% (compliance) |
| `EXPORTER_DEBUG` | no | `true` | Include `file` + `logging` exporters |

*If Tempo uses a self-signed cert, set `tls.insecure: true` in the
`otlp/tempo` exporter block instead of providing a CA file.

## Operator-tunable sampling knobs

### Normal-path sampling rate

Default: **1%** — appropriate for high-volume tenants (>1k req/s).

Change in `processors.tail_sampling.policies.normal_path_sample`:

```yaml
probabilistic:
  sampling_percentage: 1    # change to 0.1 for cost-critical, 10 for lower volume
```

Cost model guideline:
- > 1 000 req/s: keep at 1% — Tempo ingest cost stays manageable.
- 100–1 000 req/s: 10% gives better coverage without significant cost.
- < 100 req/s: 100% (or use the basic config with no sampler at all).
- Strict observability budget: 0.1% is the floor; below this, error
  sampling still fires at 100%, so SLO coverage is preserved.

### Slow span threshold

Default: **2 000 ms** (2 s). Change in:

```yaml
policies:
  - name: always_sample_slow
    type: latency
    latency:
      threshold_ms: 2000    # lower to 500 for tighter SLO
```

### Watchlist tenants

Add tenant IDs that must always be sampled at 100% (compliance, VIP,
incident investigation):

```yaml
# Environment variables (preferred — no redeploy)
OTEL_WATCHLIST_TENANT_1=tenant-abc
OTEL_WATCHLIST_TENANT_2=tenant-xyz
OTEL_WATCHLIST_TENANT_3=tenant-demo
```

Or hard-code values directly in the YAML for static watchlists.

### S3 prefix and partitioning

Default partition: `minute` — produces paths like:

```
s3://xiaoguai-otel-archive/otel-archive/year=2026/month=05/day=25/hour=14/minute=03/xiaoguai_traces_*.proto.snappy
```

Change `s3_partition` to `hour` or `day` for lower object count at
the cost of finer-grained query granularity.

## PII redaction policy

The `attributes/redact` processor enforces the following rules (see
inline comments in the YAML for the full audit trail):

| Category | Attributes | Action |
|----------|-----------|--------|
| Auth headers | `authorization`, `cookie`, `set-cookie` | delete |
| DB statements | `db.statement` | delete |
| Email | `user.email`, `enduser.email` | delete |
| Phone | `user.phone` | delete |
| SSN | `user.ssn` | delete |
| Payment | `payment.card_number`, `transaction.card_pan` | delete |
| User IDs | `enduser.id`, `user.name` | hash (SHA-256 prefix) |

10 attribute patterns covered. Operators can extend the list in
`processors.attributes/redact.actions` without restarting the
pipeline — a config reload suffices.

## Validation

### With otelcol installed

```bash
otelcol validate --config=deploy/otel-collector-advanced.yaml
```

### YAML structural check via Python (no dependencies)

```bash
python3 -c "
import yaml, sys

with open('deploy/otel-collector-advanced.yaml') as f:
    cfg = yaml.safe_load(f)

errors = []

# Verify all pipeline component references exist in top-level sections
for pipe_name, pipe in cfg.get('service', {}).get('pipelines', {}).items():
    for kind, section_key in [('receivers', 'receivers'), ('processors', 'processors'), ('exporters', 'exporters')]:
        declared = set(cfg.get(section_key, {}).keys())
        for ref in pipe.get(kind, []):
            if ref not in declared:
                errors.append(f'Pipeline {pipe_name}.{kind}: \"{ref}\" not declared in top-level {section_key}')

if errors:
    for e in errors:
        print('ERROR:', e)
    sys.exit(1)
else:
    print('OK — all pipeline component references resolve')
"
```

Run from the repo root. Exits 0 on success, 1 with a list of broken
references if any component referenced in `service.pipelines` is
missing from its respective top-level section.

### Docker Compose quick-start (dev mode)

```yaml
# Append to docker-compose.yml or docker-compose.override.yml
services:
  otel-collector-advanced:
    image: otel/opentelemetry-collector-contrib:0.100.0
    command: ["--config=/etc/otel/otel-collector-advanced.yaml"]
    volumes:
      - ./deploy/otel-collector-advanced.yaml:/etc/otel/otel-collector-advanced.yaml:ro
    ports:
      - "4317:4317"
      - "4318:4318"
      - "8889:8889"
      - "13133:13133"
      - "55679:55679"
    environment:
      OTEL_CLUSTER_NAME: local-dev
      OTEL_AWS_REGION: us-east-1
      OTEL_ENVIRONMENT: development
      OTEL_TEMPO_ENDPOINT: tempo:4317
      OTEL_S3_BUCKET: xiaoguai-otel-archive-dev
      OTEL_LOKI_ENDPOINT: loki:3100/otlp
      OTEL_LOKI_INSECURE: "true"
      EXPORTER_DEBUG: "true"
```

Note: the `awss3` exporter requires AWS credentials in the environment
or an instance profile. In dev, use LocalStack with
`AWS_ENDPOINT_URL=http://localstack:4566`.

## Cost model

| Signal | Volume assumption | Monthly cost estimate |
|--------|------------------|-----------------------|
| Traces (1% sample) | 1 000 req/s → 10 spans/req → 100 spans/s sampled | Tempo ingest ~260 M spans/month |
| Traces (errors 100%) | 0.1% error rate → 1 span/s → ~2.6 M spans/month | Low |
| S3 archive (snappy) | ~1 KB/span compressed → ~270 GB/month | ~$6/month at $0.023/GB |
| Metrics | 500 series @ 15 s → 2.9 M samples/month | Prometheus ingest negligible |

Operators with strict budgets: lower `sampling_percentage` to `0.1`.
Error and escalation traces remain at 100% — SLO visibility is
preserved regardless of the normal-path discount.
