# Operator runbook

Day-2 procedures for a Xiaoguai single-binary deployment (DEC-033).
Each procedure is short on prose and long on copy-paste commands; tune to
your host as needed.

Xiaoguai now ships as **one self-contained Rust binary** backed by an
**embedded SQLite database file** — there is no Postgres, no Valkey/Redis,
no Kubernetes, and no external datastore to operate. The cache is
in-process. State lives in a single file:

```
$XDG_DATA_HOME/xiaoguai/data.db    # when XDG_DATA_HOME is set
~/.xiaoguai/data.db                # otherwise (the systemd service user's home)
```

Under the packaged systemd unit the service user is `xiaoguai` and its
state lives beneath `/var/lib/xiaoguai`. There is **one implicit owner**
— no tenants, no multi-tenancy. Access (when you turn it on) is a single
static username + password over HTTP Basic.

## Bring-up

Bare-metal / systemd, from a release tarball (`.deb`/`.rpm` follow the
same shape):

```bash
curl -LO https://github.com/xiaoguai-agent/xiaoguai/releases/download/vX.Y.Z/xiaoguai-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
tar -xzf xiaoguai-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
cd xiaoguai-vX.Y.Z-x86_64-unknown-linux-gnu

sudo bash scripts/install.sh                       # provisions the xiaoguai system user + unit
sudo cp /etc/xiaoguai/config.example.yaml /etc/xiaoguai/config.yaml
sudo $EDITOR /etc/xiaoguai/config.yaml             # set auth + audit hmac (see below)
sudo systemctl enable --now xiaoguai-core
```

The unit is `xiaoguai-core.service`; its `ExecStart` is
`/usr/local/bin/xiaoguai serve --config /etc/xiaoguai/config.yaml`.
There are no Secrets to pre-create — credentials go in env vars or a
drop-in (see "Configuration & secrets"). For a throwaway localhost run
you can skip the unit entirely:

```bash
xiaoguai serve        # creates ~/.xiaoguai/data.db on first boot, runs open
```

Docker Compose users run the single service:

```bash
docker compose up -d xiaoguai      # one service; the SQLite file lives on a mounted volume
```

## Configuration & secrets

Config is a single YAML file (`/etc/xiaoguai/config.yaml` under systemd)
plus environment overrides. Every key has an `XIAOGUAI_<SECTION>__<KEY>`
env form. Keep credentials in env / a `.env` / a drop-in — **never** in a
checked-in config file.

```yaml
# /etc/xiaoguai/config.yaml
auth:
  username: owner          # empty username OR password = auth disabled (open)
  password: ""             # leave empty here; set via env below
audit:
  hmac_key: dev-only-change-me     # dev/smoke only
  signing_key_env: XIAOGUAI_AUDIT_SIGNING_KEY   # prod key read from this env var
```

```bash
# systemd drop-in: /etc/systemd/system/xiaoguai-core.service.d/override.conf
[Service]
Environment="XIAOGUAI_AUTH__USERNAME=owner"
Environment="XIAOGUAI_AUTH__PASSWORD=<owner-password>"
Environment="XIAOGUAI_AUDIT_SIGNING_KEY=<32+ byte hex>"
```

```bash
sudo systemctl daemon-reload
sudo systemctl restart xiaoguai-core
```

**Auth is a single static owner over HTTP Basic.** When both
`auth.username` and `auth.password` are non-empty, every non-public route
requires `-u "$XIAOGUAI_USER:$XIAOGUAI_PASS"`. When either is empty the
gate is **disabled** and the server runs open — fine on `localhost`, but
front it with a credential (or a reverse proxy) before binding a routable
address. `/healthz` is always public. There is no OIDC, no JWT, no
Casbin, no RBAC, and no scopes.

## Migrations

Migrations are embedded in the binary and applied **automatically on
boot** (`sqlx::migrate!` against the SQLite file). There is nothing to
run by hand. To confirm the server can open the store and pass its
bootstrap checks:

```bash
xiaoguai smoke        # exits 0 on success, non-zero on any failure
```

To inspect the applied migration state directly:

```bash
sqlite3 ~/.xiaoguai/data.db \
  "SELECT version, description, success FROM _sqlx_migrations ORDER BY version;"
```

## Rotating the audit HMAC key

The chain is signed with the key named by `audit.signing_key_env`
(default `XIAOGUAI_AUDIT_SIGNING_KEY`). Rotation:

```bash
# 1. Snapshot the DB first (so you can fall back if rotation goes wrong):
xiaoguai backup --out /var/backups/xiaoguai-pre-rotate.tar.gz

# 2. Generate the new key and set it in the drop-in (keep the old value
#    commented nearby for the verification window):
openssl rand -hex 32

# /etc/systemd/system/xiaoguai-core.service.d/override.conf
#   Environment="XIAOGUAI_AUDIT_SIGNING_KEY=<new-hex>"

# 3. Reload and restart:
sudo systemctl daemon-reload && sudo systemctl restart xiaoguai-core
```

Rows signed with the previous key still verify only while that key is
available to the verifier. Keep the old key for your verification window
(recommended: 30 days). Cutting the window short before the verifier has
re-baselined regresses verification — see the audit-chain section.

## Disaster recovery

There is exactly one piece of durable state: `data.db`. Recovery is
"restore the most recent snapshot of that file".

| Scenario                       | Procedure                                                                                     |
|--------------------------------|-----------------------------------------------------------------------------------------------|
| `data.db` lost / corrupted     | Restore the latest `xiaoguai backup` snapshot (see below) → `xiaoguai smoke` → start the unit. |
| In-process cache "lost"        | Nothing to do — the cache is in-process. A `systemctl restart xiaoguai-core` rebuilds it; no data loss. |
| Data tamper suspected          | `xiaoguai audit export …` verifies the chain in-window; a verify failure (409) = tampered chain. See "Audit chain". |
| Binary integrity in doubt      | `cosign verify …` the release artifact before reinstalling; re-pull the signed tarball.        |

Restore writes `data.db` atomically (the live file is saved as
`<path>.bak` first):

```bash
sudo systemctl stop xiaoguai-core
xiaoguai restore --in /var/backups/xiaoguai-2026-06-03.tar.gz \
  --restore-db ~/.xiaoguai/data.db --force
sudo systemctl start xiaoguai-core
```

`xiaoguai backup` produces a `.tar.gz` (optionally age-encrypted with
`--encrypt <pubkey>`) containing the SQLite snapshot + config; schedule
it from cron / a systemd timer.

## Observability quick refs

| Channel    | Endpoint / location              | Notes                                                                 |
|------------|----------------------------------|-----------------------------------------------------------------------|
| Liveness   | `GET /healthz`                   | Always 200 when the process is healthy; public (no auth).             |
| Metrics    | `GET /metrics`                   | **Opt-in.** Prometheus exposition only when built with the `observability` cargo feature (off by default). OTLP export is gated the same way. |
| Logs       | `stdout` / `journalctl -u xiaoguai-core` | JSON, structured via `tracing-subscriber`.                            |
| Audit      | `audit_log` table in `data.db`   | HMAC-chained, append-only.                                            |

The default release build does **not** expose `/metrics` or emit OTLP. If
you need them, deploy a build with `--features observability`; otherwise
those references do not apply.

## Killing a runaway session

```bash
curl -X POST http://localhost:8080/v1/sessions/<sess-id>/cancel \
  -u "$XIAOGUAI_USER:$XIAOGUAI_PASS"
```

(Drop the `-u` flag if you run with auth disabled.) The agent loop polls
the cancellation flag between iterations and before each fanout, so
cancellation latency is bounded by the slowest in-flight tool call.

## On-call escalation matrix

(Operator-specific. Fill in your rotation, paging channel, and runbook
URLs here.)

## Scheduler operations

Covers the reactive + scheduled job machinery.

### Scheduler overview

The scheduler is `xiaoguai_scheduler::JobRunner`, owned by the serving
binary and spawned on a dedicated tokio task when `[scheduler].enabled =
true`. It drives two arms inside a single `tokio::select!`:

| Arm                | What it does                                                                                  |
|--------------------|-----------------------------------------------------------------------------------------------|
| **Timer arm**      | Every `tick_interval_secs` seconds it calls `JobRepository::list_due` and fires each row.     |
| **Event-channel arm** | Drains a `tokio::sync::mpsc::Receiver<TriggerEvent>` fed by every wired `TriggerSource`.   |

Both arms route into the same `fire → audit → push` pipeline. There
is no second runtime path for reactive jobs.

Six trigger variants live in `Trigger`:

| Variant       | Category   | What fires it                                                              |
|---------------|------------|----------------------------------------------------------------------------|
| `Cron`        | scheduled  | 6-field UTC cron expression evaluated on the timer arm.                    |
| `Interval`    | scheduled  | Wall-clock interval evaluated on the timer arm.                            |
| `Proactive`   | scheduled  | Interval-driven; runs the `ProactiveChecker` and fires only on a non-empty reason. |
| `FileWatch`   | reactive   | `notify-debouncer-full` filesystem event matched against `(job_id, path)` routes. Bursts within `FILE_WATCH_DEBOUNCE_MS` (default 250 ms) are coalesced. |
| `Webhook`     | reactive   | `POST /v1/admin/scheduler/webhooks/:route_id` matched against route bindings. |
| `GitPush`     | reactive   | Data variant only; concrete source deferred (route incoming events through `Webhook` for now). |
| `DbPoll`      | reactive   | Data variant only; concrete source deferred.                                |

Note: `GitPush` and `DbPoll` are storage shapes — persisting a job
with these triggers is forward-compatible, but no source instantiates
events for them today.

Four push sinks live in `xiaoguai_scheduler::sinks`:

| Sink       | Transport                                       | Config block                  | Notes                                                                |
|------------|-------------------------------------------------|-------------------------------|----------------------------------------------------------------------|
| `Feishu`   | Reuses `xiaoguai-im-feishu::FeishuClient` + `TokenCache` | `[scheduler.sinks.feishu]`   | Renders proactive fires with a `【主动推送】<reason>` prefix.           |
| `Telegram` | `POST <base>/bot<token>/sendMessage`            | `[scheduler.sinks.telegram]`  | Renders proactive fires with a `🔔` bell prefix.                      |
| `Email`    | JSON webhook to your relay                      | `[scheduler.sinks.email]`     | No SMTP. Body includes `{ to, from, subject, body, payload }`.       |
| `Inbox`    | In-memory FIFO drained by the Today pane         | `[scheduler.sinks.inbox]`     | At-most-once across server restarts; persistence is a follow-up item. |

All sinks enforce the reason-required rule: a payload with
`is_proactive = true` and `reason` empty is refused **at the sink edge**
with `SinkError::Invalid("reason required")` before any network call
happens.

### Enabling the scheduler

Off by default. To turn it on:

```yaml
# config.yaml
scheduler:
  enabled: true
  tick_interval_secs: 30      # 30s is the default; lower in dev only
```

Or via env override:

```bash
export XIAOGUAI_SCHEDULER__ENABLED=true
export XIAOGUAI_SCHEDULER__TICK_INTERVAL_SECS=30
```

What gets spawned at boot when `enabled = true`:

- The job + job-run repositories over the shared `SQLite` pool.
- `RuntimeJobExecutor` wrapping the shared `xiaoguai_runtime::RuntimeContext`.
- A scheduled-session writer so every scheduled run pins a `session_id`
  for the audit-first console to drill into.
- A scheduler audit appender so scheduler audit rows join the same HMAC
  chain as REST + IM rows.
- `WebhookSource` (always when scheduler is on).
- `FileWatchSource` (only when `[scheduler.file_watch].enabled = true`).
- `tokio::spawn(JobRunner::run_loop(rx, Some(30s)))`.

Migration check — the scheduler tables ship in `0007_scheduled_jobs.sql`.
Migrations apply on boot, so confirm by querying the file:

```bash
sqlite3 ~/.xiaoguai/data.db \
  "SELECT version, description FROM _sqlx_migrations WHERE version = 7;"
```

The two tables created are `scheduled_jobs` and `scheduled_job_runs`
(JSON columns are `TEXT`; there are no `tenant_id` columns or row-level
security — this is a single-owner store).

### Configuring push sinks

Each sink reads its config from `[scheduler.sinks.<name>]`. Fields
are stored as opaque JSON in `xiaoguai-config` to avoid a
crate-graph cycle; the serving binary deserialises into the typed
config struct at sink-construction time. Example:

```yaml
scheduler:
  enabled: true
  sinks:
    feishu:
      app_id: cli_a1b2c3d4
      chat_id: oc_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
      # app_secret is intentionally NOT here — see below
    telegram:
      bot_token_env: XIAOGUAI_TG_BOT_TOKEN
      chat_id: "-1001234567890"
    email:
      webhook_url: https://relay.internal/send
      from: xiaoguai@example.com
      to: ops@example.com
    inbox: {}      # toggle on; no other config
```

**Credentials go in env vars, never in the config file.** The
production secret convention:

| Sink     | Secret env var                                   | How it's read                                                                          |
|----------|--------------------------------------------------|----------------------------------------------------------------------------------------|
| Feishu   | `XIAOGUAI_FEISHU_APP_SECRET`                     | Read by `FeishuPushSink::from_env` and stamped into the `TokenCache`.                  |
| Telegram | the env var named in `bot_token_env`             | Read at sink-construction time; the config carries the *name*, not the *value*.        |
| Email    | none required (the relay handles auth)           | Use a per-environment relay URL behind your VPC if you need transport-level auth.      |
| Inbox    | none                                             | In-process.                                                                            |

Under systemd, set these in the unit drop-in
(`/etc/systemd/system/xiaoguai-core.service.d/override.conf`) alongside
the auth + audit-key env vars, then `daemon-reload` + `restart`.

To bind a sink to a job, list the sink id in the job's `sinks`
column:

```json
{ "id": "...", "sinks": ["feishu", "inbox"], "trigger": {"type":"cron", "expr":"0 8 * * * *"}, ... }
```

Sink ids match the keys above. Unknown sink ids are silently skipped
(so a deployment that hasn't wired Telegram yet doesn't crash a job
listing both).

### Webhook source

Endpoint:

```
POST /v1/admin/scheduler/webhooks/:route_id
Content-Type: application/json
Authorization: Basic <owner-credential>   # omit when auth is disabled

<arbitrary JSON body>

→ 202 Accepted { "delivered": N }     # N = jobs fired
→ 404 Not Found                       # no jobs bound to this route_id
→ 503 Service Unavailable             # WebhookSource not wired (scheduler off)
```

The route lives under `/v1/admin/*`, so it is gated by the same single
owner Basic-auth credential as the rest of the API (or open when no
credential is set). External integrators (GitHub push events, Slack
event subscriptions) hit it with the owner credential:

```bash
curl -X POST http://localhost:8080/v1/admin/scheduler/webhooks/github-push-main \
  -u "$XIAOGUAI_USER:$XIAOGUAI_PASS" \
  -H 'content-type: application/json' -d '{"ref":"refs/heads/main"}'
```

If you need to expose the route to third parties without sharing the
owner credential, front it with your own reverse proxy that injects the
Basic credential, or run a tiny adapter that re-publishes whitelisted
events.

To bind a route to a job, create a `ScheduledJob` whose trigger is
`{"type":"webhook","route_id":"github-push-main"}` and POST your
external event JSON to the matching path. The same route id can be bound
to multiple jobs — every match fires.

### File-watch source

Off by default. To enable:

```yaml
scheduler:
  enabled: true
  file_watch:
    enabled: true
    load_routes_from_db: true     # default true; scans scheduled_jobs at boot
    routes:
      # Static, config-defined routes — ops-friendly, no DB write needed
      - job_id: notes-reindex
        path: /var/notes
      - job_id: configs-watch
        path: /etc/xiaoguai/templates
```

Route discovery merges two sources:

1. **Static config routes** above.
2. **DB-defined routes** — every enabled row in `scheduled_jobs`
   whose trigger type is `file_watch`. The
   `JobRepository::list_reactive()` SQL filter is
   `trigger->>'type' IN ('file_watch','webhook','git_push','db_poll')`.

Static routes win on `(job_id, path)` conflict. A bad path (missing
file, permission denied) logs at `error` and continues — one
misconfigured route does not kill the source.

Caveats:

- **Debounce window.** The source uses `notify-debouncer-full` to
  coalesce bursts of OS events into a single `TriggerEvent` per logical
  change. The debounce window defaults to **250 ms** and is controlled
  by the env var:

  ```
  FILE_WATCH_DEBOUNCE_MS=250   # integer milliseconds; 0 disables coalescing
  ```

  A `git checkout` over a thousand-file repo that previously fired
  thousands of `TriggerEvent`s now fires one per debounce window.
  If you need faster reactions (e.g. a CI hot-reload scenario), lower
  the value; if you still see spurious duplicate jobs under very bursty
  operations, raise it.

- **Access-only events are dropped.** `inotify`'s `IN_ACCESS` /
  fsevent's read events fire on every read and would saturate. The
  source filters to create / data-modify / name-modify / remove
  kinds only.

### Proactive triggers

A proactive job ticks on `interval_secs` and runs a check-prompt
through a cheap model. The job only fires when the checker returns a
non-empty reason.

Trigger shape in the JSON `scheduled_jobs.trigger` column:

```json
{
  "type": "proactive",
  "check_prompt": "Anything notable in the last hour's PRs?",
  "interval_secs": 1800
}
```

Two non-negotiables are enforced in the runner:

1. **Daily fire budget**, default 3 fires/day. Configurable via
   `RunnerOptions::budget_limit_per_user_per_day`; today the binary
   picks `DEFAULT_PROACTIVE_BUDGET_PER_DAY = 3`. Exhausting the budget
   produces a `scheduler.proactive_denied` audit row so the Today pane
   can show *why* proactive pings stopped.
2. **Reason required on push payloads.** The runner threads the
   checker's reason into every audit row and the push payload. The
   four real sinks refuse delivery on `is_proactive = true` +
   `reason` empty before any network call.

**Fail-safe defaults — "no checker installed ⇒ no fires" is
intentional.** Until a real cheap-model checker is wired, the binary
leaves `JobRunner::with_proactive_checker(...)` unset and proactive jobs
tick silently. Same story for the budget ledger: missing ledger ⇒ no
fires. A misconfigured deployment must not be able to bypass the budget
by leaving a piece out.

To wire a custom `ProactiveChecker` today, you need a code change in
the binary — the trait is:

```rust
#[async_trait]
trait ProactiveChecker: Send + Sync {
    async fn should_fire(
        &self,
        prompt: &str,
        ctx: ProactiveCtx,
    ) -> Result<Option<String>, ProactiveError>;
}
```

`Some(reason)` ⇒ fire with that reason. `None` ⇒ skip silently.
`Err(_)` ⇒ skip silently for this tick.

Wire it via `RuntimeJobExecutor` construction:

```rust
let runner = JobRunner::new(jobs, runs, executor, audit_appender)
    .with_proactive_checker(Arc::new(MyHaikuChecker::new(llm_router)))
    .with_budget_ledger(Arc::new(InMemoryBudgetLedger::with_default_limit()));
```

Exposing the checker as a config knob (so an operator can flip
backends without recompiling) is a future item.

### Audit chain

Every fired attempt — proactive, reactive, or scheduled — writes an
`audit_log` row keyed:

| Field      | Value                                                            |
|------------|------------------------------------------------------------------|
| `actor`    | `scheduler:<job_id>` (one stable string, prefix-recognisable)    |
| `action`   | `scheduler.job_run` on every attempt; `scheduler.proactive_denied` on budget exhaustion |
| `resource` | The job id repeated, for filter consistency with non-scheduler rows |
| `details`  | JSON merging `{ outcome, attempt, ... }` with any reactive-source detail under `trigger` |

There is **one HMAC chain for the whole instance** (single owner). The
chain doesn't distinguish scheduler-actor rows from user-actor rows —
they're entries in the same append-only chain.

Verify the chain in-window via the compliance export, which runs chain
verification before rendering and exits non-zero (409) on a break:

```bash
xiaoguai audit export \
  --framework soc2 \
  --from 2026-01-01T00:00:00Z --to 2026-06-03T00:00:00Z \
  --output /tmp/audit-bundle.json
# → exits 0 + writes the bundle when the chain is intact
# → exits non-zero (409) with a machine-readable error when the chain is broken
```

You can also inspect rows directly in the store:

```bash
sqlite3 ~/.xiaoguai/data.db \
  "SELECT id, ts, actor, action FROM audit_log ORDER BY id DESC LIMIT 20;"
```

**A broken chain means the on-disk `audit_log.hmac` for some row
doesn't match the recomputed value.** The two common causes:

1. **Operator `DELETE`/`UPDATE` against `audit_log`.** The most common
   cause. The chain links each row's `hmac` to the previous row's
   `prev_hmac`; a deleted or edited row leaves a gap and every
   subsequent verify fails. The audit chain is **append-only by
   design**; never `DELETE` or `UPDATE` rows in `audit_log`. If you need
   to redact a row, append a tombstone follow-up entry and leave the
   original in place. PII is scrubbed (emails, IPv4, `Bearer` tokens,
   AWS keys) **before** signing — see
   [`local-memory-and-redaction.md`](local-memory-and-redaction.md) §3
   (`XIAOGUAI_AUDIT_REDACT_PII`, on by default).
2. **HMAC key rotation without the verification window.** See the
   "Rotating the audit HMAC key" section above. While both keys are
   available the verifier accepts entries signed by either key; cut the
   window short and verifications regress.

Recovery: identify the broken row id from the export error, pull that
row plus the two adjacent rows out of `data.db`, and investigate. There
is no automated repair — the chain is the source of truth. If you need
to discard the break, restore the latest pre-break `xiaoguai backup`.

### Troubleshooting

Five scenarios from real operational history:

**1. Stuck job (long-running executor).** A scheduled run sits in
`status = 'running'` for hours.

```bash
# Find the run row:
sqlite3 ~/.xiaoguai/data.db \
  "SELECT id, job_id, started_at, attempt FROM scheduled_job_runs
   WHERE status = 'running' AND started_at < datetime('now','-30 minutes');"

# Cancel via the session it pinned (every scheduled run pins a session):
curl -X POST "http://localhost:8080/v1/sessions/$SESS_ID/cancel" \
  -u "$XIAOGUAI_USER:$XIAOGUAI_PASS"
```

If the executor isn't honouring cancellation (e.g. blocked on a sync
network call), `systemctl restart xiaoguai-core`; the run row stays
`running` until manual cleanup. The runner doesn't time-out runs
automatically; that's a future item.

**2. Runaway proactive budget.** A misconfigured checker fires every
tick. The budget ledger is in-memory today, keyed on `(user_id, day_utc)`
— for the single owner that's effectively one daily counter.

```bash
# Restart the service to drain the in-memory ledger:
sudo systemctl restart xiaoguai-core

# Then disable the offending job until you've fixed the checker:
sqlite3 ~/.xiaoguai/data.db \
  "UPDATE scheduled_jobs SET enabled = 0 WHERE id = '$JOB_ID';"
```

A persistent ledger is deferred; until it ships, a service restart is
the drain knob. The `scheduler.proactive_denied` rows in `audit_log`
tell you when the budget was blown.

**3. Webhook 404.** `POST /v1/admin/scheduler/webhooks/:route_id`
returns 404.

The route_id has no jobs bound. Verify:

```bash
sqlite3 ~/.xiaoguai/data.db \
  "SELECT id, enabled FROM scheduled_jobs
   WHERE json_extract(trigger, '\$.type') = 'webhook'
     AND json_extract(trigger, '\$.route_id') = '$ROUTE_ID';"
```

Common causes: typo in `route_id`, job disabled (`enabled = 0`), or
scheduler off (`[scheduler].enabled = false`) so the `WebhookSource` was
never wired — in which case you'd get 503 instead. Note the route only
fires `enabled = true` jobs; a disabled job binding still counts but
won't fire.

**4. Audit chain break.** `xiaoguai audit export` exits non-zero with a
409 chain-verification error.

```bash
# Identify the broken row id from the export error JSON, then pull the
# adjacent rows out of the store:
sqlite3 ~/.xiaoguai/data.db \
  "SELECT id, ts, actor, action, hex(hmac), hex(prev_hmac) FROM audit_log
   WHERE id BETWEEN $BROKEN_AT - 1 AND $BROKEN_AT + 1;"
```

The most common cause is a manual `DELETE`/`UPDATE` against `audit_log`.
The second most common is HMAC key rotation that cut the verification
window short. The chain is append-only and there is no automated
repair — investigate, then decide whether to accept the break or restore
the latest pre-break `xiaoguai backup`.

**5. File-watch missing events on macOS.** Events fire for files at
`/var/notes/...` but the job never runs.

macOS's fsevent backend resolves `/var` to `/private/var`. If the
job's trigger is configured with the symlinked path (`/var/notes`)
but the source sees the canonical path (`/private/var/notes`),
events match nothing. Register routes using the **canonical path**.

```bash
# Confirm the canonical form:
python3 -c "import os; print(os.path.realpath('/var/notes'))"
# /private/var/notes
```

Update the job's trigger or the `[scheduler.file_watch].routes`
entry to use the canonical path and the events match. This is
macOS-specific; Linux inotify and Windows ReadDirectoryChangesW
don't have the redirect.

## Sessions + IM history operations

The IM gateway and the scheduler both write into `sessions` + `messages`
in `data.db`. Two knobs an operator should know:

**`[im].use_in_process_history` — escape hatch.** Default `false`. When
`false`, the IM gateway reads/writes conversation history through the
`SQLite` store (the `data.db` row is the source of truth, so history
survives restarts). When `true`, the gateway keeps an in-process
`ConversationHistory` instead — handy for ephemeral debugging, but
history is then lost on restart.

```bash
# Ephemeral dev / debug only — history not persisted:
export XIAOGUAI_IM__USE_IN_PROCESS_HISTORY=true
```

Because this is a single-instance deployment there is no multi-replica
consistency concern; the only trade-off is durability across restarts.

**`[im].max_messages_per_conversation` — replay cap.** Default 50.
The IM history reader assembles at most this many trailing turns into
the agent's input — older messages stay in `data.db` for audit but are
not replayed into the agent. Bump this for use cases where the agent
needs a longer per-conversation context window; mind the LLM token
budget.

**Synthetic session per scheduled run.** Every fired `ScheduledJob`
writes a `sessions` row with:

| Field        | Value                                            |
|--------------|--------------------------------------------------|
| `id`         | Fresh UUID v4, stamped into `scheduled_job_runs.session_id`. |
| `user_id`    | `scheduler:<job_id>` — stable, prefix-recognisable.          |
| `title`      | `scheduled: <job.name>`                                       |
| `model`      | `job.payload.model` (falls back to `"scheduler"`).            |
| `status`     | `active`                                                      |

The scheduled-session writer runs **after** the runtime completes, so a
writer failure can't cancel an already-completed agent run — the LLM
work isn't wasted. The audit-first console joins
`scheduled_job_runs.session_id → sessions.id → messages` to drill from a
scheduled-run row into the chat-style transcript.

Filter scheduler-driven sessions out of regular chat-ui listings by
the `scheduler:` user_id prefix:

```sql
SELECT * FROM sessions WHERE user_id NOT LIKE 'scheduler:%';
```

Inversely, find every session for a specific job:

```sql
SELECT s.id, s.created_at, s.title
FROM sessions s
WHERE s.user_id = 'scheduler:' || ?   -- bind the job id
ORDER BY s.created_at DESC;
```
