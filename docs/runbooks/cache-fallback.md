# Cache fallback: in-process vs Valkey/Redis

Xiaoguai's cache layer (rate-limiting buckets, IM session locks, smoke
heartbeats, etc.) supports two interchangeable backends. The active mode is
chosen at boot from a single knob: `cache.url`.

## The two modes

| Mode          | When selected                                                    | Storage             | Survives restart | Multi-replica safe |
| ------------- | ---------------------------------------------------------------- | ------------------- | ---------------- | ------------------ |
| Redis/Valkey  | `cache.url` starts with `redis://` or `rediss://`               | external broker     | yes              | yes                |
| In-process    | `cache.url` is empty or any other scheme (e.g. `memory://`)     | DashMap in heap     | no               | no                 |

At startup the binary emits one of:

```
cache: connected to Redis/Valkey                        # Redis mode
cache: in-process backend (no Redis/Valkey URL configured)   # fallback mode
```

The `xiaoguai smoke` subcommand likewise logs `smoke: cache: in-process (no
round-trip)` in the fallback path — the set/get round-trip is skipped because
it would only prove that an in-memory map works.

## When to use which

**Use the in-process fallback when:**

- single-tenant, single-process deploy (e.g. an air-gapped enterprise box
  where shipping Valkey adds operational weight for no gain)
- local development without a Valkey container running
- ephemeral demo / smoke environments

**Use Redis/Valkey when:**

- two or more replicas of `xiaoguai serve` share rate-limit buckets or IM
  session locks
- you need cache state to survive process restarts (e.g. long-lived
  scheduler webhook nonces)
- regulatory or SRE requirements demand an out-of-process state store

## Configuring the knob

`config.yaml`:

```yaml
cache:
  url: ""                  # empty → in-process fallback
  key_prefix: "xiaoguai:"
```

```yaml
cache:
  url: "redis://localhost:6379/0"   # Redis/Valkey mode
  key_prefix: "xiaoguai:"
```

Environment override (per the config layering rules):

```bash
XIAOGUAI_CACHE__URL=""             # force in-process
XIAOGUAI_CACHE__URL=redis://h:6379 # force Redis
```

## Operational notes

- The in-process backend honors sub-second TTLs verbatim. The Redis backend
  clamps TTLs to 1 s (Redis `EX` granularity).
- The in-process store is per-`Cache`-instance — two `Cache::connect("",
  ...)` calls inside the same process return independent maps. Production
  wiring constructs exactly one `Cache` per service.
- Switching modes requires a restart. There is no live failover from Redis
  to in-process; if Redis becomes unreachable the affected operations
  return `CacheError::Redis` and callers handle it.
- Migration path: when scaling from one replica to multi-replica, stand up
  Valkey, set `XIAOGUAI_CACHE__URL=redis://…`, restart. No data migration
  is needed because the in-process state is intentionally ephemeral
  (rate-limit windows, session locks, smoke heartbeats — none of which
  must persist).
