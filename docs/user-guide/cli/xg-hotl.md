# xg hotl *(planned for v1.3)*

> **Implementation status**: The HOTL policy store and enforcer are fully
> implemented in `xiaoguai-api/src/hotl/` and wired into the REST API at
> `GET|POST|DELETE /v1/hotl/policies` (v1.2.3). The `xg hotl` CLI wrapper
> does **not yet exist**; this page describes the intended interface.
> Source: `crates/xiaoguai-api/src/hotl/policy.rs`,
> `crates/xiaoguai-api/src/hotl/enforcer.rs`.

## SYNOPSIS

```
xg hotl [GLOBAL-FLAGS] <SUBCMD> [SUBCMD-FLAGS] [ARGS]
```

## DESCRIPTION

`xg hotl` administers Human-on-the-Loop (HOTL) budget policies. Institutional
AI deployments use HOTL to bound LLM spend, invocation counts, and other
action categories within a rolling time window. When a limit is breached the
enforcer either escalates to a human via an IM channel or denies the action
outright (fail-closed).

The `check` subcommand lets operators test a policy evaluation without
triggering a real action, which is useful for smoke-testing before deploying
a new policy.

## GLOBAL FLAGS

| Flag | Env | Default | Description |
|------|-----|---------|-------------|
| `--config <PATH>` | `XIAOGUAI_CONFIG` | `~/.xiaoguai/config.yaml` | YAML config file |
| `--token <TOKEN>` | `XIAOGUAI_API_TOKEN` | — | Bearer token |
| `--api-base <URL>` | `XIAOGUAI_API_BASE` | `http://localhost:7600` | API server base URL |
| `--output <FORMAT>` | — | `table` | `json` \| `yaml` \| `table` |

## SUBCOMMANDS

| Subcommand | Description |
|-----------|-------------|
| `policy create` | Create a new HOTL budget policy for a tenant |
| `policy list` | List policies for a tenant (optionally filtered by scope) |
| `policy get` | Fetch a single policy by id |
| `policy update` | Update mutable fields of an existing policy |
| `policy delete` | Delete a policy by id |
| `check` | Run a one-shot budget check against the live enforcer |

---

### xg hotl policy create

```
xg hotl policy create --tenant-id <UUID> --scope <SCOPE>
    --window-secs <N> [--max-count <N>] [--max-usd <F>]
    [--escalate-to <DEST>]
```

| Flag | Required | Description |
|------|:--------:|-------------|
| `--tenant-id <UUID>` | yes | Tenant the policy applies to |
| `--scope <SCOPE>` | yes | Action category (e.g. `llm_call`, `email_send`, `webhook_invoke`) |
| `--window-secs <N>` | yes | Rolling window width in seconds |
| `--max-count <N>` | no | Maximum invocation count within the window |
| `--max-usd <F>` | no | Maximum cumulative USD cost within the window |
| `--escalate-to <DEST>` | no | IM channel or email to notify on breach; if absent the action is denied outright |

At least one of `--max-count` or `--max-usd` must be supplied.

**Example — cap LLM calls to 100/hour, escalate to Slack:**

```
$ xg hotl policy create \
    --tenant-id 550e8400-e29b-41d4-a716-446655440000 \
    --scope llm_call \
    --window-secs 3600 \
    --max-count 100 \
    --escalate-to feishu:#ops-alerts

id: a1b2c3d4-0000-0000-0000-000000000001
tenant_id: 550e8400-e29b-41d4-a716-446655440000
scope: llm_call
window_seconds: 3600
max_count: 100
max_usd: ~
escalate_to: feishu:#ops-alerts
```

**Example — hard spend cap, no escalation (deny on breach):**

```
$ xg hotl policy create \
    --tenant-id 550e8400-e29b-41d4-a716-446655440000 \
    --scope llm_call \
    --window-secs 86400 \
    --max-usd 5.00

id: a1b2c3d4-0000-0000-0000-000000000002
tenant_id: 550e8400-e29b-41d4-a716-446655440000
scope: llm_call
window_seconds: 86400
max_count: ~
max_usd: 5.0
escalate_to: ~
```

---

### xg hotl policy list

```
xg hotl policy list --tenant-id <UUID> [--scope <SCOPE>]
```

| Flag | Required | Description |
|------|:--------:|-------------|
| `--tenant-id <UUID>` | yes | Filter by tenant |
| `--scope <SCOPE>` | no | Further filter by action category |

**Example:**

```
$ xg hotl policy list --tenant-id 550e8400-e29b-41d4-a716-446655440000

ID                                    SCOPE      WINDOW   MAX_COUNT   MAX_USD   ESCALATE_TO
a1b2c3d4-0000-0000-0000-000000000001  llm_call   3600 s   100         -         feishu:#ops-alerts
a1b2c3d4-0000-0000-0000-000000000002  llm_call   86400 s  -           $5.00     -
```

---

### xg hotl policy get

```
xg hotl policy get --id <UUID>
```

| Flag | Required | Description |
|------|:--------:|-------------|
| `--id <UUID>` | yes | Policy id to fetch |

**Example:**

```
$ xg hotl policy get --id a1b2c3d4-0000-0000-0000-000000000001 --output json
{
  "id": "a1b2c3d4-0000-0000-0000-000000000001",
  "tenant_id": "550e8400-e29b-41d4-a716-446655440000",
  "scope": "llm_call",
  "window_seconds": 3600,
  "max_count": 100,
  "max_usd": null,
  "escalate_to": "feishu:#ops-alerts"
}
```

---

### xg hotl policy update

*(planned for v1.3 — mutable fields: `max_count`, `max_usd`, `escalate_to`,
`window_secs`; `scope` and `tenant_id` are immutable after creation)*

```
xg hotl policy update --id <UUID> [--max-count <N>] [--max-usd <F>]
    [--escalate-to <DEST>] [--window-secs <N>]
```

---

### xg hotl policy delete

```
xg hotl policy delete --id <UUID>
```

| Flag | Required | Description |
|------|:--------:|-------------|
| `--id <UUID>` | yes | Policy id to remove |

Returns `204 No Content` from the API; the CLI prints `deleted <id>`.

**Example:**

```
$ xg hotl policy delete --id a1b2c3d4-0000-0000-0000-000000000002
deleted a1b2c3d4-0000-0000-0000-000000000002
```

---

### xg hotl check

```
xg hotl check --tenant-id <UUID> --scope <SCOPE> --amount <F>
```

Runs a one-shot enforcer check without dispatching a real agent action. Useful
for verifying that a newly created policy behaves as expected.

| Flag | Required | Description |
|------|:--------:|-------------|
| `--tenant-id <UUID>` | yes | Tenant context |
| `--scope <SCOPE>` | yes | Action category |
| `--amount <F>` | yes | Simulated cost/count increment (use `1.0` for invocation counting) |

The enforcer is **optimistic**: it records the event before returning the
verdict. Use this subcommand against a non-production tenant to avoid
polluting real usage logs.

**Example — within budget:**

```
$ xg hotl check \
    --tenant-id 550e8400-e29b-41d4-a716-446655440000 \
    --scope llm_call \
    --amount 1.0

verdict: Allow
```

**Example — budget breached with escalation:**

```
$ xg hotl check \
    --tenant-id 550e8400-e29b-41d4-a716-446655440000 \
    --scope llm_call \
    --amount 1.0

verdict: Escalate
reason: "count limit 100 reached in window 3600 s — escalating to feishu:#ops-alerts"
```

**Example — hard deny:**

```
verdict: Deny
reason: "USD limit $5.00 exceeded in window 86400 s — no escalation target configured"
```

## EXIT CODES

| Code | Meaning |
|------|---------|
| 0 | Success (verdict `Allow` or `Escalate`) |
| 1 | Generic error (network, auth failure) |
| 2 | Invalid arguments |
| 64 | Policy not found |
| 65 | Verdict `Deny` — use in CI pipelines to detect unexpected denials |

## SEE ALSO

- REST API: `GET|POST|DELETE /v1/hotl/policies`
- Source: `crates/xiaoguai-api/src/hotl/`
- ADR: `docs/decisions/` (budget enforcement design)
- Runbook: `docs/runbooks/`
