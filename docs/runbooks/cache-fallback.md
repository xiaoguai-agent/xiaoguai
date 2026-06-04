# Cache: in-process only (no external cache)

Xiaoguai's cache layer (smoke heartbeats, IM session locks, scheduler webhook
nonces, etc.) runs **entirely in-process**. There is no Redis, no Valkey, no
external broker. After the single-user SQLite pivot (DEC-033) each person runs
their own single-binary instance, so an out-of-process shared cache buys
nothing — there is only ever one process to share with.

## The one mode

| Mode       | Storage         | Survives restart | Notes                                  |
| ---------- | --------------- | ---------------- | -------------------------------------- |
| In-process | DashMap in heap | no               | the only backend; nothing to configure |

At startup the binary emits:

```
smoke: cache: in-process
```

The `xiaoguai smoke` subcommand logs `smoke: cache: in-process`
— the set/get round-trip is skipped because it would only prove that an
in-memory map works.

## Why in-process is the right (and only) default

- Single-owner, single-process deploy: each user runs one `xiaoguai serve`
  reachable over their own URL. There are no replicas to coordinate, so
  shared cache state is meaningless.
- Cached values are intentionally ephemeral — smoke heartbeats, IM session
  locks, scheduler webhook nonces. None of them must persist across a
  restart; losing them on restart is correct behaviour.
- Durable state lives in the embedded SQLite database file
  (`data.db`), not in the cache. The cache never holds the system of record.

## Configuration

There is nothing to point at — the cache has no external dependency. The
`cache` config block, if present, only carries a key prefix:

```yaml
cache:
  key_prefix: "xiaoguai:"   # optional; namespaces in-heap keys
```

The `cache.url` field was removed entirely. Any Redis/Valkey URL in an older config is
ignored; if you are migrating from a pre-pivot deployment, you may delete the
`cache.url` line and tear down any Redis/Valkey container — it is no longer
used.

## Operational notes

- The in-process backend honors sub-second TTLs verbatim (no external broker
  granularity clamping).
- The in-process store is per-`Cache`-instance — two `Cache::new(...)`
  calls inside the same process return independent maps. Production wiring
  constructs exactly one `Cache` per service.
- Restarting `xiaoguai serve` clears the cache. This is expected: rate-limit
  windows, session locks, and smoke heartbeats are recomputed on demand, and
  durable data is read back from `data.db`.
- There is no live failover concept — there is no second backend to fail over
  to. If you previously relied on cache state surviving a restart, move that
  data into the SQLite database instead.
