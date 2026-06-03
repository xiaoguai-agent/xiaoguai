# Per-environment setup playbook

Covers bringing up Xiaoguai in **dev**, **staging**, and **prod**. After the
single-user SQLite pivot (DEC-033) Xiaoguai is a single self-contained Rust
binary with an embedded SQLite database file — there is no Postgres, no
Redis/Valkey, and no other external datastore. Each person runs their own
instance reachable over their own URL. Rust source is not touched here; for
source-level build steps see `deploy/README.md`.

Adjacent runbooks referenced throughout:
- `docs/runbooks/cache-fallback.md` — the in-process cache (no external cache)
- `docs/runbooks/slo.md` — latency / error objectives
- `docs/runbooks/disaster-recovery-wave3.md` — backup / restore of `data.db`
- `docs/runbooks/hotl-escalation-stuck.md` — HotL troubleshooting
- `docs/runbooks/pack-install-troubleshoot.md` — skill pack diagnostics
- `docs/runbooks/outcome-chain-debug.md` — outcomes chain debug

---

## Table of Contents

1. [Dev — local quickstart](#1-dev--local-quickstart)
2. [Staging](#2-staging)
3. [Prod](#3-prod)
4. [Common pitfalls](#4-common-pitfalls)

---

## 1. Dev — local quickstart

### Prerequisites

| Tool | Version |
|------|---------|
| rustup / Rust toolchain | 1.91+ (`rust-toolchain.toml` pins the exact version) |
| pnpm | ≥ 9 (only if building the web UI) |

No database server, cache server, or container runtime is required — the
binary embeds SQLite and the cache runs in-process. Docker Compose is
optional and only bundles the single binary for convenience.

### Bring up the stack

The simplest path is to run the binary directly:

```bash
# Build and run; the binary creates its SQLite file on first start.
cargo run --bin xiaoguai -- serve
```

It listens on port 7600 and creates its database at
`$XDG_DATA_HOME/xiaoguai/data.db` (or `~/.xiaoguai/data.db` if `XDG_DATA_HOME`
is unset). Override the data directory with `XIAOGUAI_DATA_DIR` if you want it
elsewhere.

The optional compose file at `deploy/docker-compose.yml` runs the same single
binary in a container:

```bash
docker compose -f deploy/docker-compose.yml up -d
```

Expected containers / processes:

| Process | Port | Purpose |
|---------|------|---------|
| `xiaoguai` (`xiaoguai serve`) | 7600 | API server + embedded SQLite |

There is no separate `postgres`, `redis`/`valkey`, `otel-collector`,
`prometheus`, or `grafana` to run. Metrics/tracing are opt-in behind the
`observability` cargo feature (off by default) — build with that feature only
if you actually need `/metrics` or OTLP.

### Apply migrations

Migrations run automatically at server startup via SQLx against the embedded
SQLite database. There is nothing to apply by hand. To confirm the schema is
current, just start the binary and watch for `All migrations applied` in the
log, or restart it:

```bash
docker compose -f deploy/docker-compose.yml restart xiaoguai
```

If you ever need to inspect the database directly, point the `sqlite3` CLI at
the data file:

```bash
sqlite3 "${XDG_DATA_HOME:-$HOME/.local/share}/xiaoguai/data.db" \
  "SELECT version, description FROM _sqlx_migrations ORDER BY version;"
```

### Seed demo data

If a seed script ships with your checkout, run it against the running
instance:

```bash
bash scripts/seed-demo.sh
```

The script seeds a sample set of agent outcome records, installs a sample
skill pack, and registers a HotL policy for the single owner. (There are no
tenants to create — the instance has exactly one owner.)

### Smoke test order

Run each step and confirm it returns success before proceeding. With auth
unset (empty username/password) the local instance is open on localhost; if
you set `auth.username`/`auth.password`, pass them via HTTP Basic:

```bash
BASE="http://localhost:7600"
# If auth is configured, add: -u "$XIAOGUAI_AUTH_USERNAME:$XIAOGUAI_AUTH_PASSWORD"

# 1. API health
curl -sf "$BASE/healthz" | grep ok

# 2. Record an outcome
curl -sf -X POST "$BASE/v1/outcomes" \
  -H "Content-Type: application/json" \
  -d '{"agent_name":"smoke","kind":"task_complete","value":1}' \
  | jq .id

# 3. Create a HotL policy
curl -sf -X POST "$BASE/v1/hotl/policies" \
  -H "Content-Type: application/json" \
  -d '{"scope":"smoke","max_count":10,"window_seconds":60}' \
  | jq .id

# 4. Install a skill pack
curl -sf -X POST "$BASE/v1/skills/install" \
  -H "Content-Type: application/json" \
  -d '{"pack_slug":"incident-triage"}' \
  | jq .id

# 5. Start a watch
curl -sf -X POST "$BASE/v1/watch" \
  -H "Content-Type: application/json" \
  -d '{"scope":"smoke"}' \
  | jq .watch_id

# 6. Trigger anomaly run
curl -sf -X POST "$BASE/v1/anomaly/run" \
  -H "Content-Type: application/json" \
  -d '{}' \
  | jq .status
```

### Tear down

```bash
docker compose -f deploy/docker-compose.yml down
```

There are no `postgres` or `redis` volumes to purge — the only durable state
is the SQLite `data.db` file. To start completely fresh, stop the binary and
delete that file:

```bash
rm -f "${XDG_DATA_HOME:-$HOME/.local/share}/xiaoguai/data.db"
```

Back it up first with `xiaoguai backup` if you want to keep the data.

---

## 2. Staging

Staging mirrors prod configuration but holds no real production data. It is
still a single binary with its own `data.db`. HotL is enforced and outcomes
recording is on.

### Install

Staging runs the same single binary as prod, just with a separate data
directory and (optionally) its own auth credentials. Install the package
(`.deb` / `.rpm` / tarball) or run the container, then start the service:

```bash
# systemd (package install): the unit runs `xiaoguai serve`.
sudo systemctl enable --now xiaoguai

# or container:
docker run -d --name xiaoguai-staging -p 7600:7600 \
  -v xiaoguai-staging-data:/var/lib/xiaoguai \
  -e XIAOGUAI_AUTH__USERNAME="$STAGING_AUTH_USER" \
  -e XIAOGUAI_AUTH__PASSWORD="$STAGING_AUTH_PASS" \
  -e XIAOGUAI_AUDIT__HMAC_KEY="$(openssl rand -hex 32)" \
  ghcr.io/xiaoguai-agent/xiaoguai:latest
```

Auth is a single static owner over HTTP Basic. Set `auth.username` /
`auth.password` in `config.yaml`, or the env vars
`XIAOGUAI_AUTH__USERNAME` / `XIAOGUAI_AUTH__PASSWORD`. Leaving them empty
serves an open instance — acceptable for a localhost-only dev box, not for a
shared staging host. There is no OIDC, JWT, Casbin, RBAC, or scopes to
configure.

The HMAC audit-chain key (`XIAOGUAI_AUDIT__HMAC_KEY`, 32 bytes hex) is the
only other secret and is unchanged by the pivot.

### Observability (optional)

`/metrics` and OTLP export are opt-in behind the `observability` cargo
feature, off by default. Only build/run the observability-enabled binary if
you need them; otherwise skip this entirely.

### Load test

After the service is up and `/healthz` returns `ok`, run the mixed workload:

```bash
k6 run scripts/loadtest/k6/scenarios/mixed.js \
  --env BASE_URL="https://staging.xiaoguai.example.com" \
  --vus 20 --duration 5m
```

### Acceptance criteria before promoting to prod

- [ ] `GET /healthz` returns `ok`
- [ ] Error rate (5xx) < 0.5% sustained over the load test
- [ ] Latency p95 ≤ target (see `docs/runbooks/slo.md`)
- [ ] Migrations confirmed applied (start log shows `All migrations applied`)
- [ ] HotL policies enforced (test with a policy that immediately blocks — see `hotl-escalation-stuck.md`)
- [ ] Outcomes chain intact (run `outcome-chain-debug.md` step 1 on the load-test session ID)
- [ ] `xiaoguai backup` produces a restorable snapshot (verify per `disaster-recovery-wave3.md` §1)
- [ ] k6 run exits with 0 failures

---

## 3. Prod

Prod defaults are conservative. HotL and outcomes recording are enabled
gradually after a 24-hour soak. Prod is still one binary, one `data.db` — no
HA, no replicas, no multi-region. Resilience comes from regular
`xiaoguai backup` snapshots and the ability to restore `data.db`.

### Pre-flight checklist

- [ ] Backup completed — run `xiaoguai backup` (see `disaster-recovery-wave3.md` §1)
- [ ] Rollback plan documented: keep the previous package version / container image so you can reinstall and restore the pre-upgrade `data.db` snapshot
- [ ] Change window approved by on-call lead
- [ ] `docs/runbooks/hotl-escalation-stuck.md` open in a tab — most likely failure mode post-deploy

### Apply migrations

Migrations run at startup automatically. Restart the service to apply any
pending migration:

```bash
sudo systemctl restart xiaoguai
sudo systemctl status xiaoguai
```

SQLite migrations 0011/0012/0015 are additive (new tables or new columns with
defaults). On a single-process instance there is no rolling upgrade — there is
a brief restart while the new binary starts and applies migrations. Take a
`xiaoguai backup` immediately before restarting so you can roll back the data
file if a migration misbehaves.

### Phased rollout

Because there is a single owner and a single process, "rollout" is feature
enablement on the one instance rather than fleet-wide staging.

**Phase 1 — deploy (deploy day)**

Install/upgrade the binary with HotL and outcomes recording off:

```yaml
# config.yaml
agent:
  hotl:
    enabled: false
outcomes:
  recording: false
```

Restart and confirm `/healthz` is `ok`.

**Phase 2 — outcomes recording on (after the instance is stable)**

Set `outcomes.recording: true` and restart.

**Phase 3 — 24-hour soak**

Monitor error rate and p95 latency against the targets in
`docs/runbooks/slo.md`. Do not proceed to phase 4 if either is breached.

**Phase 4 — HotL on low-risk scopes**

Enable HotL for read-only or low-impact agent scopes first:

```bash
curl -sf -X POST "https://api.xiaoguai.example.com/v1/hotl/policies" \
  -u "$PROD_AUTH_USER:$PROD_AUTH_PASS" \
  -H "Content-Type: application/json" \
  -d '{
    "scope": "incident-triage",
    "max_count": 5,
    "window_seconds": 3600,
    "escalate_to": "oncall@example.com"
  }'
```

Expand to additional scopes once no HotL escalations are stuck (see
`docs/runbooks/hotl-escalation-stuck.md`).

### Post-deploy verification

```bash
BASE="https://api.xiaoguai.example.com"
AUTH="-u $PROD_AUTH_USER:$PROD_AUTH_PASS"

# API health
curl -sf "$BASE/healthz" | grep ok

# Outcomes recording
curl -sf $AUTH "$BASE/v1/outcomes/summary" | jq .total

# HotL policies in force
curl -sf $AUTH "$BASE/v1/hotl/policies" | jq 'length'
```

Confirm:
- Error rate < 0.1%
- p95 latency within the target declared in `docs/runbooks/slo.md`
- A fresh `xiaoguai backup` succeeds after the upgrade

---

## 4. Common pitfalls

### Missing auth / HMAC config — instance refuses to start or serves open

**Symptom:** the service exits at startup with `missing environment variable`
for the audit HMAC key, or it starts but is reachable without credentials on a
shared host.

**Fix:** set `XIAOGUAI_AUDIT__HMAC_KEY` (exactly 32 bytes hex-encoded) on any
non-throwaway instance. On a shared/staging/prod host also set
`XIAOGUAI_AUTH__USERNAME` / `XIAOGUAI_AUTH__PASSWORD` (or the `auth.username` /
`auth.password` config keys) so the single owner is protected by HTTP Basic.
Empty auth is only acceptable for a localhost-only dev box.

---

### Data directory not writable — startup fails on `data.db`

**Symptom:** the binary exits with a SQLite "unable to open database file" or
"attempt to write a readonly database" error.

**Diagnosis:**

```bash
# Confirm the resolved data directory exists and is writable by the service user.
ls -ld "${XDG_DATA_HOME:-$HOME/.local/share}/xiaoguai"
```

**Fix:** ensure the data directory (`$XDG_DATA_HOME/xiaoguai` or
`~/.xiaoguai`, or `XIAOGUAI_DATA_DIR` if set) exists and is owned by the user
running `xiaoguai serve`. The systemd unit's service user must own
`/var/lib/xiaoguai`.

---

### Migration order — 0011 applied without 0012

**Symptom:** service starts, but HotL policy creation returns `500`; logs show
`column hotl_policies.window_seconds does not exist`.

**Cause:** migration 0012 adds `window_seconds` to the `hotl_policies` table.
It depends on the table created by 0011. If 0012 was not applied (e.g. the
process crashed mid-startup and the restart was skipped), the column is
absent.

**Fix:**

```bash
# Confirm which migrations ran
sqlite3 "${XDG_DATA_HOME:-$HOME/.local/share}/xiaoguai/data.db" \
  "SELECT version FROM _sqlx_migrations ORDER BY version;"

# If 0012 is missing, restart the service — SQLx applies the pending migration
sudo systemctl restart xiaoguai
sudo systemctl status xiaoguai
```

Do not manually apply migrations with the `sqlite3` CLI — let SQLx manage the
sequence to avoid checksum mismatches.

---

### Skill pack install row recorded but no activation

**Symptom:** `POST /v1/skills/install` returns `200` with an `id`, the row
appears in the `skill_packs` table, but the pack's tools are not visible to
agents and `GET /v1/skills/installed` lists the pack with no active tools.

**Cause:** the DB row is written and the install is acknowledged, but the
runtime tool-loader hot-reload path may not be wired for the pack version
installed.

**Workaround:** restart the service after installing packs to load them at
startup:

```bash
sudo systemctl restart xiaoguai
```

See `docs/runbooks/pack-install-troubleshoot.md` for full diagnostics
including the `409 Conflict` and `404 pack not in catalog` cases.
