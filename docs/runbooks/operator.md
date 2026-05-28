# Operator runbook

Day-2 procedures for Xiaoguai v1.0 deployments. Each procedure is short
on prose and long on copy-paste commands; tune to your cluster as
needed.

## Bring-up

Helm-based:

```bash
helm install xiaoguai deploy/helm/xiaoguai \
  --create-namespace --namespace xiaoguai \
  --set image.tag=v1.0.0 \
  --values your-values.yaml
```

`your-values.yaml` must reference four pre-created Secrets — see
`deploy/helm/xiaoguai/values.yaml` for the keys each must contain.

## Migrations

The binary runs `xiaoguai-storage::migrations` on startup. To inspect
the applied state:

```bash
kubectl exec -it deploy/xiaoguai -- /usr/local/bin/xiaoguai-core smoke
```

`smoke` connects to every dependency and exits non-zero on failure.

## Rotating the audit HMAC key

```bash
# 1. Export the chain end-pointer:
kubectl exec deploy/xiaoguai -- xiaoguai admin audit head > prev.json

# 2. Create the new secret:
kubectl create secret generic xiaoguai-audit-next \
  --from-literal=hmac_key="$(openssl rand -hex 32)"

# 3. Rolling upgrade with the new secret name:
helm upgrade xiaoguai deploy/helm/xiaoguai \
  --set secrets.audit=xiaoguai-audit-next \
  --reuse-values

# 4. Keep both secrets around for the verification window
#    (recommended: 30 days). The audit verifier accepts entries signed
#    by either key during that window.
```

## Disaster recovery

| Scenario                                | Procedure                                                                |
|-----------------------------------------|--------------------------------------------------------------------------|
| Postgres lost                           | Restore latest backup → run `xiaoguai-core smoke` → roll pods.            |
| Valkey lost                             | Cache; restart pods. No data loss.                                       |
| Tenant data leak suspected              | `xiaoguai admin audit verify --tenant <id>` — chain inconsistency = tamper. |
| Image registry compromised              | `cosign verify ...` before redeploy; revoke + re-sign latest tag.         |

## Observability quick refs

| Channel          | Endpoint                            | Notes                                  |
|------------------|-------------------------------------|----------------------------------------|
| Liveness         | `GET /healthz`                      | Always 200 when the process is healthy. |
| Metrics          | `GET /metrics` (v0.6.1)             | Prometheus exposition.                  |
| Logs             | `stdout`                            | JSON, structured via `tracing-subscriber`. |
| Audit            | `audit_log` table                   | HMAC-chained.                           |

## Killing a runaway session

```bash
curl -X POST http://xiaoguai-core.svc:8080/v1/sessions/<sess-id>/cancel \
  -H 'authorization: Bearer <operator-jwt>'
```

The agent loop polls the registry token between iterations + before each
fanout, so cancellation latency is bounded by the slowest in-flight tool
call.

## On-call escalation matrix

(Operator-specific. Fill in your rotation, paging channel, and runbook
URLs here.)

## Scheduler operations

Added in v1.0.1 to cover everything that landed in the v0.10.x →
v0.12.x band.

### Scheduler overview

The scheduler is `xiaoguai_scheduler::JobRunner`, owned by
`xiaoguai-core` and spawned on a dedicated tokio task when
`[scheduler].enabled = true`. It drives two arms inside a single
`tokio::select!`:

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
| `Inbox`    | In-memory FIFO drained by the v0.11.1 Today pane | `[scheduler.sinks.inbox]`     | At-most-once across server restarts; persistence is a v0.12.x.1 item. |

All sinks enforce the reason-required rule from roadmap §5.5: a
payload with `is_proactive = true` and `reason` empty is refused
**at the sink edge** with `SinkError::Invalid("reason required")`
before any network call happens.

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

- `PgJobRepository` + `PgJobRunRepository` over the existing PG pool.
- `RuntimeJobExecutor` wrapping the shared `xiaoguai_runtime::RuntimeContext`.
- `PgScheduledSessionWriter` (v0.12.1) so every scheduled run pins a
  `session_id` for the audit-first console to drill into.
- `PgSchedulerAuditAppender` adapter over the existing `PgAuditSink`
  so scheduler audit rows join the same HMAC chain as REST + IM rows.
- `WebhookSource` (always when scheduler is on).
- `FileWatchSource` (only when `[scheduler.file_watch].enabled = true`).
- `tokio::spawn(JobRunner::run_loop(rx, Some(30s)))`.

Migration check — the scheduler tables ship in
`0007_scheduled_jobs.sql` (added in v0.10.0). Confirm the migration
applied:

```bash
kubectl exec deploy/xiaoguai -- /usr/local/bin/xiaoguai-core smoke
# ...or directly:
kubectl exec deploy/xiaoguai -- psql "$DATABASE_URL" -c \
  "SELECT version, installed_on FROM _sqlx_migrations WHERE version = 7;"
```

The two tables created are `scheduled_jobs` and `scheduled_job_runs`,
both RLS-scoped on `tenant_id` with the same `current_setting(
'app.current_tenant_id', true)` predicate as the rest of v0.6.1's
tenant-aware tables.

### Configuring push sinks

Each sink reads its config from `[scheduler.sinks.<name>]`. Fields
are stored as opaque JSON in `xiaoguai-config` to avoid a
crate-graph cycle; `xiaoguai-core` deserialises into the typed
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

Helm: stash each secret in a Kubernetes Secret + mount via `envFrom`.
The deployment chart's `values.yaml` already follows this pattern for
the audit HMAC key (`secrets.audit`); add `secrets.scheduler` the
same way.

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
Authorization: Bearer <operator-jwt>

<arbitrary JSON body>

→ 202 Accepted { "delivered": N }     # N = jobs fired
→ 404 Not Found                       # no jobs bound to this route_id
→ 503 Service Unavailable             # WebhookSource not wired (scheduler off)
```

Today (v0.12.0–v0.12.2) the route is **admin-bearer-gated** — same
gate as the rest of `/v1/admin/*`, enforced by the Casbin
middleware. External integrators (GitHub push events, Slack event
subscriptions) can't hit it directly without an operator-issued
admin JWT.

**Per-tenant API tokens for this route are deferred to v0.12.x.1.**
Until that ships, the operational pattern is one of:

- Front the route with your own reverse-proxy auth that converts a
  per-tenant token to the admin JWT.
- Run a tiny adapter inside your cluster that holds the admin JWT
  and re-publishes whitelisted events.

To bind a route to a job, create a `ScheduledJob` whose trigger is
`{"type":"webhook","route_id":"github-push-main"}` and POST your
external event JSON to
`/v1/admin/scheduler/webhooks/github-push-main`. The same route id
can be bound to multiple jobs — every match fires.

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

- **Debounce window (v1.1.10+).** The source now uses
  `notify-debouncer-full` to coalesce bursts of OS events into a
  single `TriggerEvent` per logical change.  The debounce window
  defaults to **250 ms** and is controlled by the env var:

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

Two non-negotiables from roadmap §5.5 are enforced in the runner:

1. **Per-user-per-day budget**, default 3 fires/day. Configurable via
   `RunnerOptions::budget_limit_per_user_per_day`; today the operator
   binary picks `DEFAULT_PROACTIVE_BUDGET_PER_DAY = 3`. Exhausting
   the budget produces a `scheduler.proactive_denied` audit row so
   the Today pane can show *why* a tenant stopped getting pings.
2. **Reason required on push payloads.** The runner threads the
   checker's reason into every audit row and the push payload. The
   four real sinks refuse delivery on `is_proactive = true` +
   `reason` empty before any network call.

**Fail-safe defaults — "no checker installed ⇒ no fires" is
intentional.** Until v1.1 wires a real cheap-model checker, the
operator binary leaves `JobRunner::with_proactive_checker(...)`
unset and proactive jobs tick silently. Same story for the budget
ledger: missing ledger ⇒ no fires. A misconfigured deployment must
not be able to bypass the budget by leaving a piece out.

To wire a custom `ProactiveChecker` today, you need a code change in
the operator binary — the trait is:

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
backends without recompiling) is a v1.1 item.

### Audit chain

Every fired attempt — proactive, reactive, or scheduled — writes an
`audit_log` row keyed:

| Field      | Value                                                            |
|------------|------------------------------------------------------------------|
| `actor`    | `scheduler:<job_id>` (one stable string, prefix-recognisable)    |
| `action`   | `scheduler.job_run` on every attempt; `scheduler.proactive_denied` on budget exhaustion |
| `resource` | The job id repeated, for filter consistency with non-scheduler rows |
| `details`  | JSON merging `{ outcome, attempt, ... }` with any reactive-source detail under `trigger` |

The HMAC chain doesn't distinguish scheduler-actor rows from
user-actor rows — they're entries in the same per-tenant chain.

Verify with the v0.6.5 endpoint:

```bash
curl -s -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/admin/audit/verify?tenant_id=$TENANT"
# → 200 { "valid": true, "head_sequence": 4711, "head_hmac": "..." }
# → 200 { "valid": false, "broken_at_sequence": 3392, ... }
```

**A broken chain means the on-disk `audit_log.hmac` for some row
doesn't match the recomputed value.** The two common causes:

1. **Operator `DELETE` against `audit_log`.** The most common cause.
   The chain links each row's HMAC to the previous row's HMAC; a
   deleted row leaves a gap and every subsequent verify fails. The
   audit chain is **append-only by design**; never `DELETE` or
   `UPDATE` rows in `audit_log`. If you need to redact a row,
   replace its `details` with a tombstone via a follow-up entry and
   leave the original in place. To prevent PII from reaching the chain
   in the first place, audit entries are scrubbed (emails, IPv4,
   `Bearer` tokens, AWS keys) **before** signing — see
   [`local-memory-and-redaction.md`](local-memory-and-redaction.md) §3
   (`XIAOGUAI_AUDIT_REDACT_PII`, on by default).
2. **HMAC key rotation without the verification window.** See the
   "Rotating the audit HMAC key" section above. During the 30-day
   window the verifier accepts entries signed by either key; cut the
   window short and verifications regress.

Recovery: identify the broken sequence number from the verify
response, pull the row + the two adjacent rows out of PG, and
investigate. There is no automated repair — the chain is the source
of truth.

### Troubleshooting

Five scenarios from real operational history:

**1. Stuck job (long-running executor).** A scheduled run sits in
`status = 'running'` for hours.

```bash
# Find the run row:
psql "$DATABASE_URL" -c \
  "SELECT id, job_id, started_at, attempt FROM scheduled_job_runs
   WHERE status = 'running' AND started_at < now() - interval '30 minutes';"

# Cancel via the session it pinned (v0.12.1 wired the synthetic session):
curl -X POST "http://xiaoguai-core.svc:8080/v1/sessions/$SESS_ID/cancel" \
  -H "Authorization: Bearer $OPERATOR_JWT"
```

If the executor isn't honouring cancellation (e.g. blocked on a
sync network call), restart the pod; the run row stays `running`
until manual cleanup. The runner doesn't time-out runs automatically;
that's a v1.1 item.

**2. Runaway proactive budget.** A misconfigured checker fires every
tick. The budget ledger is in-memory today, keyed on `(user_id, day_utc)`.

```bash
# Restart the pod to drain the in-memory ledger:
kubectl rollout restart deploy/xiaoguai

# Then disable the offending job until you've fixed the checker:
psql "$DATABASE_URL" -c \
  "UPDATE scheduled_jobs SET enabled = false WHERE id = '$JOB_ID';"
```

The v0.12.0 PG-backed ledger is deferred; until it ships, a pod
restart is the drain knob. The `scheduler.proactive_denied` rows in
`audit_log` tell you which tenant blew the budget.

**3. Webhook 404.** `POST /v1/admin/scheduler/webhooks/:route_id`
returns 404.

The route_id has no jobs bound. Verify:

```bash
psql "$DATABASE_URL" -c \
  "SELECT id, enabled FROM scheduled_jobs
   WHERE trigger->>'type' = 'webhook'
     AND trigger->>'route_id' = '$ROUTE_ID';"
```

Common causes: typo in `route_id`, job disabled (`enabled = false`),
or scheduler off (`[scheduler].enabled = false`) so the
`WebhookSource` was never wired — in which case you'd get 503
instead. Note that the route only fires `enabled = true` jobs; a
disabled job binding still counts but won't fire.

**4. Audit chain break.** `GET /v1/admin/audit/verify` returns
`valid: false`.

```bash
# Identify the broken sequence:
curl -s -H "Authorization: Bearer $ADMIN_JWT" \
  "http://xiaoguai-core.svc:8080/v1/admin/audit/verify?tenant_id=$TENANT" | jq

# Pull the adjacent rows:
psql "$DATABASE_URL" -c \
  "SELECT sequence, actor, action, hmac, prev_hmac FROM audit_log
   WHERE tenant_id = '$TENANT'
     AND sequence BETWEEN $BROKEN_AT - 1 AND $BROKEN_AT + 1;"
```

The most common cause is a manual `DELETE` against `audit_log`. The
second most common is HMAC key rotation that cut the verification
window short. The chain is append-only and there is no automated
repair — investigate, decide whether to accept the break (and
re-baseline the verifier's expected head pointer) or restore from
backup.

**5. File-watch missing events on macOS.** Events fire for files at
`/var/notes/...` but the job never runs.

macOS's fsevent backend resolves `/var` to `/private/var`. If the
job's trigger is configured with the symlinked path (`/var/notes`)
but the source sees the canonical path (`/private/var/notes`),
events match nothing. The v0.10.1 integration test canonicalises
explicitly; the operator-visible knob is: register routes using the
**canonical path**.

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

The IM gateway (v0.7.x) and the scheduler (v0.12.1) both write into
`sessions` + `messages`. Two knobs an operator should know:

**`[im].use_in_process_history` — escape hatch.** Default `false`.
When `false`, the IM gateway uses `PgImHistoryStore` so multi-replica
webhooks stay consistent (any pod can answer any conversation; the
PG row is the source of truth). When `true`, the gateway keeps the
v0.7.2 in-process `ConversationHistory` instead.

```bash
# Single-replica dev / debug only:
export XIAOGUAI_IM__USE_IN_PROCESS_HISTORY=true
```

**Never turn this on in production HA deployments.** A second
replica fielding the same conversation will read empty history and
re-introduce duplicate turns.

**`[im].max_messages_per_conversation` — replay cap.** Default 50.
The IM history store reads at most this many trailing turns when
assembling the agent's input — older messages stay in the DB for
audit but are not replayed into the agent. Bump this for use cases
where the agent needs a longer per-conversation context window;
mind the LLM token budget.

**Synthetic session per scheduled run (v0.12.1).** Every fired
`ScheduledJob` writes a `sessions` row with:

| Field        | Value                                            |
|--------------|--------------------------------------------------|
| `id`         | Fresh UUID v4, stamped into `scheduled_job_runs.session_id`. |
| `user_id`    | `scheduler:<job_id>` — stable, prefix-recognisable.          |
| `title`      | `scheduled: <job.name>`                                       |
| `model`      | `job.payload.model` (falls back to `"scheduler"`).            |
| `status`     | `Active`                                                       |

The `PgScheduledSessionWriter` runs **after** the runtime completes,
so a writer failure can't cancel an already-completed agent run —
the LLM work isn't wasted. The audit-first console (v0.11.1) joins
`scheduled_job_runs.session_id → sessions.id → messages` to drill
from a scheduled-run row into the chat-style transcript.

Filter scheduler-driven sessions out of regular chat-ui listings by
the `scheduler:` user_id prefix:

```sql
SELECT * FROM sessions WHERE user_id NOT LIKE 'scheduler:%';
```

Inversely, find every session for a specific job:

```sql
SELECT s.id, s.created_at, s.title
FROM sessions s
WHERE s.user_id = 'scheduler:' || $1   -- $1 = job id
ORDER BY s.created_at DESC;
```

If a scheduled job's `tenant_id` is `NULL` the writer bails with a
clear error and the job-run row's `error_message` carries it — RLS
gives no useful behaviour for null-tenant sessions and the
audit-first console can't surface them. Bind scheduled jobs to a
tenant.
