# Pyroscope continuous profiling — setup and flame-graph guide

**Feature flag:** `profiling` on `xiaoguai-core` (optional workspace dep)  
**Related observability:** Prometheus/Grafana (`chore/grafana-wave3`), Tempo traces, Loki logs

---

## Why continuous profiling

Ad-hoc profiling (`perf record`, `cargo flamegraph`, `pprof`) requires you to
reproduce the problem locally. Production issues — LLM call p99 spikes, lock
contention under tenant load, serde overhead on large streaming responses —
often vanish the moment you connect a debugger.

Continuous profiling keeps a 1–5 % overhead sampler running in-process at all
times. Every 60 s (default) a compressed pprof snapshot is pushed to the
Pyroscope server. You can slice by tenant, pack, endpoint, or xiaoguai version
after the fact, without having to reproduce the spike.

| Approach | Overhead | Reproduction needed | Slice by label |
|---|---|---|---|
| `cargo flamegraph` / `perf record` | 5–20 % | Yes | No |
| pprof HTTP endpoint | 0 % at rest, ~15 % during capture | Yes | No |
| Pyroscope continuous | 1–5 % always-on | No | Yes |

The LLM-call codepath is the primary motivation: provider HTTP wait and
streaming-chunk deserialization both look like "slow I/O" in a trace, but only
a flame graph shows whether the CPU time is actually in Serde, a regex, or
a `.clone()` chain.

---

## Architecture overview

```
xiaoguai-core (pprof-rs sampler, in-process)
    │  push every 60 s  (HTTP multipart/gzip)
    ▼
Pyroscope server  (pyroscope/pyroscope or Grafana Cloud Profiles)
    │
    ▼
Flame-graph UI  — slice by tag: tenant / pack / endpoint / xiaoguai_version
```

- **Agent**: Rust `pyroscope` crate wraps `pprof-rs` (frame-pointer or DWARF
  unwinding). Runs on a background thread; negligible tokio runtime impact.
- **Collection interval**: 60 s default. Reduce to 10 s for short benchmarks;
  increase to 300 s in cost-sensitive clusters.
- **Sample rate**: 100 Hz default (one stack snapshot per 10 ms). Halve to
  50 Hz to cut overhead to ~0.5 %.
- **Multi-tenancy**: every profile is tagged with `tenant`; Pyroscope's per-tenant
  label queries let you isolate one customer's workload.

---

## Rust integration steps

> **Out of scope for this runbook**: actually wiring the crate dependency.
> Implementation is tracked as a separate task.  The code below is illustrative
> only — do not add these changes to `xiaoguai-core/src/` until the task is
> opened and owned.

### Cargo.toml additions (illustrative)

```toml
# xiaoguai-core/Cargo.toml  (or workspace Cargo.toml with optional feature)
[dependencies]
pyroscope          = { version = "0.5", optional = true }
pyroscope_pprofrs  = { version = "0.5", optional = true }

[features]
profiling = ["pyroscope", "pyroscope_pprofrs"]
```

### Scaffold: `xiaoguai-core/src/profiling.rs`

The following scaffold is **included inline for reference only**.
Do not commit this file — the crate is owned by the clippy-rest agent.

```rust
// xiaoguai-core/src/profiling.rs
// SCAFFOLD — feature-gated; actual implementation tracked in #N
// Do NOT commit this file. Inline reference only.

#[cfg(feature = "profiling")]
use pyroscope::{PyroscopeAgent, PyroscopeConfig};
#[cfg(feature = "profiling")]
use pyroscope_pprofrs::{pprof_backend, PprofConfig};

/// Initialise the Pyroscope agent.
///
/// Reads `PYROSCOPE_SERVER_ADDRESS` from the environment.
/// If the variable is absent the function is a no-op — profiling disabled.
///
/// Call once from `main` before spawning the Axum server.
#[cfg(feature = "profiling")]
pub fn init_pyroscope(
    tenant: &str,
    pack: &str,
    endpoint: &str,
    xiaoguai_version: &str,
) -> Option<pyroscope::PyroscopeAgentRunning> {
    let server_address = std::env::var("PYROSCOPE_SERVER_ADDRESS").ok()?;

    let backend = pprof_backend(PprofConfig::new().sample_rate(100));

    let agent = PyroscopeAgent::builder(server_address, "xiaoguai")
        .backend(backend)
        .tags([
            ("tenant",            tenant),
            ("pack",              pack),
            ("endpoint",          endpoint),
            ("xiaoguai_version",  xiaoguai_version),
        ])
        .build()
        .expect("failed to build Pyroscope agent");

    Some(agent.start().expect("failed to start Pyroscope agent"))
}

/// No-op shim when the `profiling` feature is disabled.
#[cfg(not(feature = "profiling"))]
pub fn init_pyroscope(
    _tenant: &str,
    _pack: &str,
    _endpoint: &str,
    _xiaoguai_version: &str,
) -> Option<()> {
    None
}
```

### Tagging critical sections (illustrative)

```rust
// In the HotL check handler — adds a slice-able sub-label for flame graphs.
// Requires agent handle stored in AppState or a global OnceLock.
#[cfg(feature = "profiling")]
agent.add_tag("hotl_check", "true");

let result = hotl_enforcer.check(&policy, &ctx).await;

#[cfg(feature = "profiling")]
agent.remove_tag("hotl_check");
```

---

## Environment variables

| Variable | Default | Description |
|---|---|---|
| `PYROSCOPE_SERVER_ADDRESS` | *(absent = disabled)* | `http://pyroscope.svc:4040` in cluster |
| `PYROSCOPE_SAMPLE_RATE` | `100` | Samples per second (Hz). Lower to reduce overhead. |
| `PYROSCOPE_UPLOAD_INTERVAL_SECS` | `60` | How often profiles are pushed |
| `PYROSCOPE_AUTH_TOKEN` | *(absent)* | Bearer token for Grafana Cloud Profiles |

If `PYROSCOPE_SERVER_ADDRESS` is absent, the agent is not initialised and
zero overhead is incurred.

---

## Deployment

### Helm values — Pyroscope server

Use the upstream `grafana/pyroscope` Helm chart for self-hosted deployments:

```yaml
# pyroscope-values.yaml
pyroscope:
  replicationFactor: 1          # raise to 3 for HA
  storage:
    backend: filesystem         # or s3 / gcs / azure
    filesystem:
      dir: /data/pyroscope
  retention: 168h               # 7 days; see "Retention" below

serviceMonitor:
  enabled: true                 # scrape Pyroscope itself with Prometheus
```

```bash
helm repo add grafana https://grafana.github.io/helm-charts
helm install pyroscope grafana/pyroscope \
  --namespace observability \
  --values pyroscope-values.yaml
```

### Env vars on the xiaoguai binary

xiaoguai ships as a single binary (DEC-033) — there is no Helm chart. Point it
at your Pyroscope server through its process environment. Under systemd, add to
the unit (e.g. `/etc/systemd/system/xiaoguai.service`):

```ini
[Service]
Environment=PYROSCOPE_SERVER_ADDRESS=http://127.0.0.1:4040
Environment=PYROSCOPE_SAMPLE_RATE=100
Environment=PYROSCOPE_UPLOAD_INTERVAL_SECS=60
```

Use the Pyroscope service DNS instead of `127.0.0.1:4040` if Pyroscope runs in a
cluster (e.g. `http://pyroscope.observability.svc.cluster.local:4040`). Then
`systemctl daemon-reload && systemctl restart xiaoguai`. For a foreground run,
`export` the same vars before `xiaoguai serve`.

Build the binary with the profiling feature enabled:

```bash
cargo build --release -p xiaoguai-cli --features "observability,profiling"
```

### Storage sizing

| Load level | Profile size / pod / day | Notes |
|---|---|---|
| Light (< 10 req/s) | ~2 GB | Default 60 s interval |
| Medium (10–100 req/s) | ~10 GB | Increase interval to 120 s to halve this |
| Heavy (> 100 req/s) | ~50 GB | Consider Grafana Cloud Profiles or object storage backend |

### Retention and archiving

```yaml
# pyroscope-values.yaml
pyroscope:
  retention: 168h               # 7 days on-disk (default)

# For longer retention, push to S3:
  storage:
    backend: s3
    s3:
      bucket: xiaoguai-profiles
      region: us-east-1
      endpoint: ""              # empty = AWS; set for MinIO/Ceph
      access_key_id: ""         # prefer IRSA / workload identity
      secret_access_key: ""
```

Lifecycle rule: archive profiles older than 7 days to S3 Glacier Instant
Retrieval. Cost at 10 GB/pod/day: ~$0.04/GB/month on standard S3.

---

## Flame-graph reading guide — wave-3 endpoints

Open the Pyroscope UI at `http://pyroscope.observability.svc:4040` (or
Grafana Cloud Profiles). Select the `xiaoguai` application. Use the label
filters to narrow the view.

### HotL check (`tag: hotl_check=true`)

**Filter:** `{endpoint="/v1/hotl/check"}`

| Frame to look for | Meaning | Fix |
|---|---|---|
| `sqlx::pool::Pool::acquire` wide frame | PG connection pool exhausted | Increase `hotl_db_pool_size` |
| `serde_json::de` | Deserialising the policy blob on every check | Cache parsed policy in `AppState` |
| `ring::hmac::verify` | HMAC verification on every call | Expected; < 1 % is normal |

Wide `tokio::runtime::task::harness` at root = lock contention; look for a
`std::sync::Mutex` or `RwLock` frame just below it.

### Outcome record (`tag: endpoint=/v1/outcomes`)

**Filter:** `{endpoint="/v1/outcomes"}`

| Frame to look for | Meaning | Fix |
|---|---|---|
| `sqlx::query::Query::execute` dominant | Write path serialised through single PG connection | Use `INSERT ... RETURNING` + connection pool |
| `outcome_chain::sign` wide | HMAC signing of every outcome | Batch-sign on flush, not per-record |
| `tokio::sync::Mutex::lock` | Writer lock contention | Switch to a lock-free ring-buffer queue |

### Anomaly detector (`tag: pack=anomaly`)

**Filter:** `{pack="anomaly"}`

The z-score hot loop (`compute_z_score`) should be < 2 % of wall time for a
single detector. If it is wider:

- Check `min_count` — low values cause warmup thrashing.
- Look for `Vec::clone` inside the rolling-window accumulator; switch to a
  circular buffer (`VecDeque` with capacity pre-allocated).
- `f64::sqrt` in a tight loop? Pre-compute the reciprocal.

### LLM call (`tag: endpoint=/v1/sessions`)

**Filter:** `{endpoint="/v1/sessions"}`

Expected profile shape: > 70 % of wall time in `tokio::net::TcpStream::poll_read`
(provider HTTP wait). That is correct — we are I/O bound.

Investigate if CPU frames dominate instead:

| Frame | Meaning | Fix |
|---|---|---|
| `serde_json::ser` wide | Serialising the full prompt on every chunk | Buffer and flush at stream end |
| `String::clone` or `Vec::clone` in `build_messages` | Cloning message history per call | Switch to `Arc<[Message]>` |
| `reqwest::async_impl::client` → TLS handshake | Provider not reusing connections | Enable `reqwest` connection pooling (`pool_max_idle_per_host`) |

---

## Common findings and fixes

### Excessive `.clone()` on outcome chains

**Flame indicator:** `alloc::string::String::clone` or `outcome_chain::OutcomeChain::clone`
appears in > 3 % of samples on the write path.

**Fix:** wrap the chain in `Arc<OutcomeChain>` and clone the `Arc` instead.

```rust
// Before
fn record(&self, chain: OutcomeChain) { self.store(chain.clone()); }

// After
fn record(&self, chain: Arc<OutcomeChain>) { self.store(Arc::clone(&chain)); }
```

### Lock contention on `hotl_usage_log` writer

**Flame indicator:** `std::sync::Mutex<T>::lock` sits under the HotL handler
at > 5 % of samples; threads stacking on the same mutex.

**Fix:** size the PG connection pool to match concurrency:

```yaml
# config.yaml
hotl:
  db_pool_min: 4
  db_pool_max: 16   # at least 2× expected concurrent HotL checks
```

Or replace the in-process mutex with a `tokio::sync::Semaphore`.

### Serde JSON parse cost dominating LLM responses

**Flame indicator:** `serde_json::de::Deserializer` in > 10 % of samples on
the session endpoint, even for streaming responses.

**Fix:** parse only the `delta` field from each SSE chunk rather than
deserialising the full provider response envelope:

```rust
// Use a lightweight struct matching only the fields you need
#[derive(Deserialize)]
struct ChunkDelta { content: Option<String> }
```

Alternatively, switch to `simd-json` for the hot deserialization path
(benchmark first — gains vary by payload shape).

---

## Troubleshooting

**Pyroscope agent panics on startup with "failed to get stack trace"**  
Most likely a musl libc environment (Alpine-based image). The `pprof-rs` backend
requires frame-pointer support compiled in. Rebuild with:

```bash
RUSTFLAGS="-C force-frame-pointers=yes" cargo build --features profiling
```

Or switch the base image to `debian-slim`.

**Profiles not appearing in the UI**  
1. Verify `PYROSCOPE_SERVER_ADDRESS` is reachable from the pod:
   ```bash
   kubectl exec -it deploy/xiaoguai -- curl -s http://pyroscope.observability.svc:4040/ready
   # Expect: {"status":"ready"}
   ```
2. Check the xiaoguai pod logs for `[pyroscope]` lines at startup.
3. Confirm the binary was built with `--features profiling` — if the flag is
   absent the agent is a compile-time no-op and emits no logs.
4. Tags must match exactly; a typo in `tenant` means the profile exists but
   does not appear under your filter.

**High overhead (> 5 % CPU increase)**  
Reduce the sample rate:

```yaml
extraEnv:
  - name: PYROSCOPE_SAMPLE_RATE
    value: "50"   # 50 Hz instead of 100 Hz
```

Or increase the upload interval to 120 s to reduce compression work.

**`ERROR: profile too large to ingest`**  
Default ingest limit is 4 MB. For binaries with many symbols:

```yaml
# pyroscope-values.yaml
pyroscope:
  ingesterConfig:
    maxProfileSize: 16777216   # 16 MB
```

---

## Integration with existing observability

Pyroscope is the fourth observability signal alongside Prometheus, Tempo, and
Loki. The recommended investigation flow connects all four:

```
Prometheus histogram  →  Tempo trace  →  Loki log line  →  Pyroscope flame
```

### Step 1 — Prometheus exemplar to Tempo trace

In the Grafana wave-3 dashboard (`chore/grafana-wave3`), the
`xiaoguai_http_request_duration_seconds` histogram has exemplar links enabled.
Click a high-latency data point on the p99 panel to jump directly to the
matching Tempo trace ID.

```promql
# Find sessions with p99 > 2 s in the last 5 min
histogram_quantile(0.99,
  sum by (le, endpoint) (
    rate(xiaoguai_http_request_duration_seconds_bucket{endpoint="/v1/sessions"}[5m])
  )
)
```

### Step 2 — Tempo trace to Pyroscope flame

In the Tempo trace view, note the wall-clock start and end timestamps of the
slow span. Switch to Pyroscope and set the time range to that window:

- Application: `xiaoguai`
- Tag filter: `tenant=<tenant-from-trace>`, `endpoint=/v1/sessions`
- Time range: `[span_start − 30 s, span_end + 30 s]`

Pyroscope's 60 s collection granularity means the flame graph will cover the
span's window even for short requests.

### Step 3 — Loki log lines to trace ID

Use the Loki branch (`chore/loki`) log panel with:

```logql
{app="xiaoguai"} |= "ERROR" | json | duration > 2s
```

The structured log lines emitted by `tracing-subscriber` include `trace_id`
when the `observability` feature is active. Copy the `trace_id` value and paste
it into the Tempo search to pull the full distributed trace.

### Summary: signal cross-reference table

| Signal | Tool | Branch / source |
|---|---|---|
| Metrics + exemplars | Prometheus + Grafana | `chore/grafana-wave3` |
| Distributed traces | Tempo (OTLP/gRPC) | `chore/grafana-wave3` + observability runbook |
| Structured logs | Loki | `chore/loki` |
| CPU flame graphs | Pyroscope | This runbook |

---

## Costs and operator decision

| Option | Overhead | Flame graphs | Cost model |
|---|---|---|---|
| **Pyroscope** (self-hosted) | 1–5 % CPU | Yes, with labels | Storage cost only (~$5/pod/month at 10 GB/day on S3) |
| **Grafana Cloud Profiles** | 1–5 % CPU | Yes, with labels | Metered by sample volume (check current pricing) |
| **Parca** | 1–5 % CPU | Yes | Self-hosted; compatible with pprof-rs push |
| **eBPF profiler** (e.g. Coroot, Polar Signals) | < 1 % | Limited Rust symbol quality | Requires privileged DaemonSet; kernel 5.8+ |

**When to choose Pyroscope:** you already run the Grafana stack
(`chore/grafana-wave3`), want per-tenant label slicing, and need > 7 days
retention. The Grafana Cloud Profiles backend requires zero additional
infrastructure.

**When to choose an eBPF profiler:** overhead budget is very tight (< 0.5 %),
or you need cross-language profiling (Rust + Python SDK in the same pod). Note
that eBPF profilers have lower Rust symbol quality when DWARF info is stripped.

**When to keep ad-hoc pprof:** the performance issue is reproducible locally,
the team is not yet running Kubernetes, or the deployment is a single systemd
unit. See the existing `observability.md` runbook for the pprof HTTP endpoint
approach.
