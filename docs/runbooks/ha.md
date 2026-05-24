# HA runbook — v1.1.4

Day-2 procedures for the v1.1.4 HA topology (PG logical replication
+ Valkey Cluster + 2× xiaoguai-core behind nginx). For the
single-node baseline see `deploy/docker-compose.yml` and the v0.6
notes in `deploy/README.md`.

This runbook is intentionally short on theory and long on the
copy-paste commands you want under fire.

## 1. Topology

```
                              ┌──────────────────────┐
                  client ───► │ nginx :7600          │
                              │  least_conn          │
                              │  retries=2 on 5xx    │
                              └──────┬───────────────┘
                                     │
                       ┌─────────────┴─────────────┐
                       ▼                           ▼
              ┌─────────────────┐         ┌─────────────────┐
              │ xiaoguai-core-1 │         │ xiaoguai-core-2 │
              │ NODE_ID=core-1  │         │ NODE_ID=core-2  │
              └────┬──────────┬─┘         └─┬──────────┬────┘
                   │          │             │          │
        writes+reads          │ cache       │   writes+reads
                   ▼          │             │          ▼
        ┌──────────────────┐  │             │  ┌──────────────────┐
        │   pg-primary     │  └──── any ────┘  │   pg-primary     │
        │ wal_level=       │       node        │ (same instance)  │
        │   logical        │       in cluster  └──────────────────┘
        │ PUBLICATION      │
        │   xiaoguai_pub   │
        │ (FOR ALL TABLES) │
        └────┬─────────────┘
       logical replication
        ┌────┴────────────────┐
        ▼                     ▼
  ┌──────────────┐     ┌──────────────┐
  │pg-subscriber-│     │pg-subscriber-│
  │      a       │     │      b       │
  │ xiaoguai_sub_a    │ xiaoguai_sub_b
  └──────────────┘     └──────────────┘
   (read-ready,         (read-ready,
    NOT routed —         NOT routed —
    v1.1.4.1)            v1.1.4.1)

  ┌─────────────────────────── Valkey Cluster ───────────────────────────┐
  │  primary slots                            replica of                 │
  │  vc-1 :6379  0–5460                       vc-4 :6379                 │
  │  vc-2 :6379  5461–10922                   vc-5 :6379                 │
  │  vc-3 :6379  10923–16383                  vc-6 :6379                 │
  │                                                                       │
  │  bootstrap: valkey-cluster-init (one-shot, --cluster-replicas 1)      │
  └───────────────────────────────────────────────────────────────────────┘
```

**Why logical replication, not streaming?** The decision is locked
(see the v1.1.4 plan doc). Short version: logical repl gives us per-
table grain + the same wire protocol we want for future
shard-routing experiments. The cost is that we manage publications
and re-`REFRESH` them after primary migrations (see §3).

**Why Valkey Cluster, not Sentinel?** Same decision: cluster gives
us horizontal sharding capability for free even though today we
have one keyspace. Sentinel would force a second migration when
the cache outgrows a single primary.

## 2. Bootstrap order

The compose file's `depends_on` already encodes this; the manual
order below is what you walk if you're bringing the stack up
service-by-service for debugging.

```bash
cd <repo root>

# 1) PG primary first — subscribers + cores all wait on it.
docker compose -f deploy/docker-compose.ha.yml up -d pg-primary

# 2) Subscribers. They run pg-subscriber-init.sh which retries
#    pg_dump --schema-only against the primary up to 60s waiting
#    for the schema to materialise. They will start *empty* if the
#    core hasn't booted yet — that's fine, see §3 for the
#    REFRESH PUBLICATION pattern.
docker compose -f deploy/docker-compose.ha.yml up -d pg-subscriber-a pg-subscriber-b

# 3) Valkey cluster nodes. All 6 in parallel.
docker compose -f deploy/docker-compose.ha.yml up -d \
  valkey-cluster-1 valkey-cluster-2 valkey-cluster-3 \
  valkey-cluster-4 valkey-cluster-5 valkey-cluster-6

# 4) valkey-cluster-init forms the cluster. One-shot; exits 0.
docker compose -f deploy/docker-compose.ha.yml up valkey-cluster-init

# 5) xiaoguai-core × 2. Each runs sqlx migrations against the
#    primary on first boot. ~20s startup grace period built into
#    the healthcheck.
docker compose -f deploy/docker-compose.ha.yml up -d xiaoguai-core-1 xiaoguai-core-2

# 6) nginx — waits for both cores to be `healthy`.
docker compose -f deploy/docker-compose.ha.yml up -d nginx

# 7) Smoke.
curl http://localhost:7600/healthz   # → ok
```

**After step 5 (first time only)** the primary now has tables but
the subscribers were bootstrapped before they existed. Tell each
subscriber to refresh:

```bash
for sub in pg-subscriber-a pg-subscriber-b; do
  docker compose -f deploy/docker-compose.ha.yml exec "$sub" \
    psql -U xiaoguai -d xiaoguai -c "
      -- Pull any new tables the primary added since subscription create.
      ALTER SUBSCRIPTION $( [ \"$sub\" = pg-subscriber-a ] && echo xiaoguai_sub_a || echo xiaoguai_sub_b ) REFRESH PUBLICATION;"
done
```

(Subsequent migrations: every time you ship a new `xiaoguai-
storage::migrations/*.sql` that adds a table, run the same REFRESH
on each subscriber. Hot tip: bake this into your release pipeline.)

## 3. Failover playbook

### 3.1 PG primary loss

Logical replication does **not** auto-promote. You promote a
subscriber by hand.

```bash
# 1) Stop the dead primary's container so it doesn't accidentally
#    come back and split-brain you.
docker compose -f deploy/docker-compose.ha.yml stop pg-primary

# 2) Pick the subscriber with the lowest lag (see §4) — usually
#    pg-subscriber-a. Promote means: stop pulling from the dead
#    primary and start accepting writes.
docker compose -f deploy/docker-compose.ha.yml exec pg-subscriber-a \
  psql -U xiaoguai -d xiaoguai -c "
    -- Drop the subscription to the dead primary (this also drops
    -- the local-side worker; remote slot lingers on the dead box).
    ALTER SUBSCRIPTION xiaoguai_sub_a DISABLE;
    ALTER SUBSCRIPTION xiaoguai_sub_a SET (slot_name = NONE);
    DROP SUBSCRIPTION xiaoguai_sub_a;

    -- Create the publication on the new primary so the *other*
    -- surviving subscriber can latch onto it.
    CREATE PUBLICATION xiaoguai_pub FOR ALL TABLES;"

# 3) Repoint xiaoguai-core. Today this is a config change + restart
#    (no in-process retry-on-primary-loss — see Deferred in plan).
#    Edit XIAOGUAI_DATABASE__URL to host=pg-subscriber-a then:
docker compose -f deploy/docker-compose.ha.yml up -d --force-recreate \
  xiaoguai-core-1 xiaoguai-core-2

# 4) Repoint the OTHER subscriber at the new primary.
docker compose -f deploy/docker-compose.ha.yml exec pg-subscriber-b \
  psql -U xiaoguai -d xiaoguai -c "
    DROP SUBSCRIPTION xiaoguai_sub_b;
    CREATE SUBSCRIPTION xiaoguai_sub_b
      CONNECTION 'host=pg-subscriber-a port=5432 dbname=xiaoguai user=xiaoguai_repl password=xiaoguai'
      PUBLICATION xiaoguai_pub
      WITH (copy_data = false, create_slot = true,
            slot_name = 'xiaoguai_sub_b_slot');"
```

**Recovery RPO/RTO target:** RPO ≤ replication lag at failure
(typically < 1s under normal load; see §4 for what to watch). RTO
is bounded by step 3 — kicking the cores — call it 30s of
unavailable writes if you're already at a terminal with the env
edit queued.

### 3.2 Valkey cluster node loss

Cluster mode handles this automatically. The replica of the lost
slot owner promotes itself within `cluster-node-timeout` (5s).

```bash
# Inspect: which node has which role?
docker compose -f deploy/docker-compose.ha.yml exec valkey-cluster-1 \
  valkey-cli -p 6379 cluster nodes

# Simulate failure of vc-1 (primary):
docker compose -f deploy/docker-compose.ha.yml stop valkey-cluster-1

# Within ~10s: vc-4 (its replica) promotes. Confirm:
docker compose -f deploy/docker-compose.ha.yml exec valkey-cluster-2 \
  valkey-cli -p 6379 cluster nodes | grep master

# Bring vc-1 back. It rejoins as a replica of vc-4.
docker compose -f deploy/docker-compose.ha.yml start valkey-cluster-1
```

**Client-side caveat:** today xiaoguai-core uses a single-node
`redis://` URL pointing at one cluster node. The `redis` crate
follows `-MOVED` redirects so reads/writes still succeed under
slot ownership shifts, **but** if the specific node in
`XIAOGUAI_CACHE__URL` itself dies, the core won't fail over to a
sibling node — it will keep dialing the dead address. Workaround
today: each core points at a *different* node (compose does this
— core-1→vc-1, core-2→vc-2) so a single node death only halves
cache availability rather than killing it outright. Permanent fix
is the `redis::cluster::ClusterClient` migration (v1.1.4.2).

### 3.3 xiaoguai-core instance loss

No action required. nginx's `max_fails=2 fail_timeout=30s` flips
the unhealthy instance out of rotation for 30s after two
consecutive 5xx responses. The healthcheck on the unhealthy
container (`xiaoguai-core smoke` every 10s) eventually marks it
back up and nginx re-includes it on the next request.

```bash
# Verify nginx routing in real time:
docker compose -f deploy/docker-compose.ha.yml exec nginx \
  wget -qO- http://127.0.0.1:7600/healthz   # → ok (local)

# Watch which backend served the last request — set a custom
# response header in nginx.conf if you need this in prod. The
# default config doesn't expose backend identity (intentional —
# leak of internal topology).
```

## 4. Lag monitoring

### 4.1 PG logical replication lag

On the primary:

```sql
-- pg_stat_replication shows one row per active subscriber.
-- write_lag / flush_lag / replay_lag are intervals.
SELECT
  application_name,
  state,
  pg_size_pretty(pg_wal_lsn_diff(sent_lsn, replay_lsn)) AS backlog_bytes,
  write_lag, flush_lag, replay_lag
FROM pg_stat_replication;
```

**Acceptable lag for our reads (once v1.1.4.1 routes RO queries
to subscribers):**

| Endpoint | Lag tolerance | Why |
|---|---|---|
| `GET /v1/admin/today` | ≤ 5s | Audit summary is human-paced; a few seconds of staleness is invisible. |
| `GET /v1/admin/sessions` | ≤ 5s | Session list view. |
| `GET /v1/admin/audit` | ≤ 30s | Audit log browse is forensic; staleness is fine, integrity is what matters. |
| `GET /v1/eval/*` (list runs) | ≤ 30s | Eval results browse. |
| anything else (writes, session detail, message stream) | n/a | Stays on primary. |

If `replay_lag > 30s` for more than 60s, alert. Likely cause:
subscriber under-resourced or a long-running query blocking apply.

### 4.2 Valkey cluster health

```bash
docker compose -f deploy/docker-compose.ha.yml exec valkey-cluster-1 \
  valkey-cli -p 6379 cluster info
```

Look for:
- `cluster_state:ok` — anything else is a hot incident.
- `cluster_slots_assigned:16384` — full slot coverage.
- `cluster_slots_ok:16384` — all slots have a serving node.
- `cluster_known_nodes:6` — all 6 alive in the gossip mesh.

```bash
# Per-node detail (role, replication offset, link state):
docker compose -f deploy/docker-compose.ha.yml exec valkey-cluster-1 \
  valkey-cli -p 6379 cluster nodes
```

Replica lag isn't directly relevant for our cache use case (we
treat the cache as soft state), but if you do want it:

```bash
docker compose -f deploy/docker-compose.ha.yml exec valkey-cluster-4 \
  valkey-cli -p 6379 info replication
# look at master_repl_offset vs slave_repl_offset
```

## 5. Backup + PITR

WAL archival pattern (configure on the primary in production —
NOT in the demo compose):

```bash
# In postgresql.conf on pg-primary:
#   archive_mode = on
#   archive_command = 'aws s3 cp %p s3://xiaoguai-wal/%f'
#
# Base backup, repeatable from any host with libpq:
pg_basebackup \
  --host=pg-primary --port=5432 \
  --username=xiaoguai_repl \
  --pgdata=/backup/$(date -u +%Y%m%dT%H%M%SZ) \
  --format=tar --gzip \
  --wal-method=stream \
  --checkpoint=fast \
  --label="nightly-$(date -u +%Y%m%d)"
```

**PITR recovery sketch:**

1. Restore the most recent base backup tarball to a fresh PG data
   directory.
2. Drop a `recovery.signal` file in `${PGDATA}`.
3. Set `restore_command = 'aws s3 cp s3://xiaoguai-wal/%f %p'` and
   `recovery_target_time = '2026-05-24 14:23:00 UTC'` in
   `postgresql.auto.conf`.
4. Start postgres. It replays WAL up to the target and pauses.
5. `pg_wal_replay_resume()` once you've confirmed state. Promote.

The subscriber pair gives you a hot redundancy floor; the WAL
archive gives you a "oops, dropped the wrong table at 14:22:30"
recovery floor. Don't skip either.

## 6. What today's code is NOT

This is what v1.1.4 ships **scaffolding** for but does **not** wire
into the running application. Each item is tracked in the v1.1.4
plan doc's Deferred section.

- **Replica-aware read routing.** xiaoguai-core opens one
  `PgPool` against `XIAOGUAI_DATABASE__URL` and uses it for both
  reads and writes. RO endpoints (Today, sessions list, audit list,
  eval list) will gain a `pool_read` config knob in v1.1.4.1 that
  routes to a subscriber. Until then, the subscribers are
  passive backups + bandwidth absorbers for the WAL stream.

- **In-process PG failover.** Loss of `pg-primary` today requires
  the manual playbook in §3.1 (promote + edit env + restart cores).
  v1.1.4.2 will add an `in_process_failover: { fallback_dsn }`
  config that lets each core retry against a known-good subscriber
  on `connection_terminated` errors.

- **Valkey cluster-aware client.** The `redis` crate today opens a
  single-node connection. `-MOVED` redirects work, but failure of
  the specific dialed node won't automatically failover client-
  side. Migration to `redis::cluster::ClusterClient` is its own
  slice (post-v1.1.4 — likely v1.1.4.2 or later, depending on
  observed pain in real deploys).

- **Per-table replication granularity.** We use `FOR ALL TABLES`
  for operational simplicity. If a future workload wants to keep
  (say) `token_usage` off the subscribers to save WAL bandwidth,
  that's a publication change — search this runbook for `xiaoguai_
  pub` and `REFRESH PUBLICATION` to find the touch points.

## 7. See also

- `deploy/docker-compose.ha.yml` — the topology, in compose form.
- `deploy/helm/xiaoguai/values-ha.yaml` — same topology, in Helm
  values form (k8s operators must bring their own PG + Valkey
  operators; chart doesn't install them).
- `docs/plans/2026-05-24-v1.1.4.md` — design + deferral list.
- `docs/runbooks/operator.md` — single-node day-2 procedures.
