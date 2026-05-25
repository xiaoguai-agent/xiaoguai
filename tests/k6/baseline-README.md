# k6 Perf Baseline — Regeneration Guide

`baseline.json` is the reference snapshot against which every CI run is
compared.  Any endpoint whose p95 latency regresses more than **20 %** from
this baseline fails the `perf-test` job.

## When to regenerate

Regenerate whenever a performance change is **intentional**:

- Deliberate latency trade-off (e.g. stronger rate-limiting, extra validation)
- Infrastructure upgrade that shifts absolute numbers (new runner size, Postgres
  version, Rust edition)
- New endpoints added to `wave3-mixed-workload.js`

Do **not** regenerate to hide an accidental regression — fix the regression
instead.

## How to regenerate

### Option A — local run (recommended for deliberate changes)

```bash
# 1. Stand up the full stack locally (postgres + redis + xiaoguai-core)
docker compose -f deploy/docker-compose.yml up -d --build

# 2. Wait for healthy
until curl -fsS http://localhost:8080/healthz; do sleep 1; done

# 3. Run the wave-3 workload and capture output
k6 run --summary-export tests/k6/baseline-new.json tests/k6/wave3-mixed-workload.js

# 4. Inspect the results — make sure numbers look reasonable
cat tests/k6/baseline-new.json | jq '.metrics | to_entries[] | {key, p95: .value.values["p(95)"]}'

# 5. Replace the checked-in baseline
cp tests/k6/baseline-new.json tests/k6/baseline.json

# 6. Commit the new baseline alongside the code change that motivated it
git add tests/k6/baseline.json
git commit -m "perf: update k6 baseline after <describe change>"
```

### Option B — promote a CI artifact

After a PR that intentionally changes perf lands and its CI run passes
(perhaps with `PERF_THRESHOLD` temporarily widened):

```bash
# Download the perf-results artifact from the Actions run
# Artifact name: perf-results-<run_id>  →  k6-results.json

cp ~/Downloads/k6-results.json tests/k6/baseline.json
git add tests/k6/baseline.json
git commit -m "perf: update k6 baseline from CI run <run_id>"
```

## Baseline format

`perf-compare.sh` accepts either:

- **k6 `--summary-export` format** (single JSON object with `.metrics`
  containing `p(95)` values) — this is what `baseline.json` uses.
- **k6 `--out json` stream format** (newline-delimited data point objects) —
  used for the live `k6-results.json` produced by CI.

`perf-compare.sh` auto-detects the format.

## Adjusting the threshold

The default threshold is **20 %**.  To tighten or loosen it project-wide,
edit the `PERF_THRESHOLD` default in `scripts/perf-compare.sh`, or pass it as
an environment variable for a one-off run:

```bash
PERF_THRESHOLD=10 bash scripts/perf-compare.sh k6-results.json tests/k6/baseline.json
```

## Related files

| File | Purpose |
|------|---------|
| `.github/workflows/perf-regression.yml` | CI job definition |
| `scripts/perf-compare.sh` | Comparator script |
| `docs/perf-budget-wave3` | Human-readable budget targets |
| `tests/k6/wave3-mixed-workload.js` | Load test script (lives on `test/k6-wave3-load`) |
