# Human-on-the-Loop Policy

Human-on-the-Loop (HotL) lets operators define spending and rate budgets for
agent actions — LLM calls, email sends, external API invocations — and receive
an escalation notification when those budgets are exceeded, rather than blocking
the agent outright. This keeps agents running at full autonomy while giving
operators a supervised safety valve.

Introduced in **v1.2.3**.

## Why a hot-path enforcer

Enterprise deployments of autonomous agents face a common tension: giving the
agent enough headroom to be useful, but being notified before runaway costs or
unexpected bursts go undetected. A naive deny-on-breach approach stops the
agent mid-task and creates support burden. HotL takes the opposite stance:

- **Allow the action** even when a budget is breached.
- **Notify the operator asynchronously** so a human can review and intervene.
- **Deny only** when the operator explicitly opted out of escalation (i.e. set
  no `escalate_to` address), or when the policy store is unreachable.

The enforcer sits inline on the LLM call path as of v1.2.3. Email-send and
webhook-invoke wiring are tracked as follow-up items.

## Policy model

Each HotL policy is one row in `hotl_policies` and controls one `scope`
over a rolling time window.

| Field | Type | Description |
|-------|------|-------------|
| `id` | UUID | Auto-generated primary key |
| `scope` | string | Action category: `llm_call`, `email_send`, `external_api`, or any custom string |
| `window_seconds` | integer | Rolling window width; must be > 0 |
| `max_count` | integer? | Maximum invocation count inside the window; `null` = no count limit |
| `max_usd` | float? | Maximum cumulative USD cost inside the window; `null` = no cost limit |
| `escalate_to` | string? | IM channel address or email to notify on breach; `null` = Deny on breach |

At least one of `max_count` or `max_usd` must be set. A policy with both fields
set fires on whichever limit is breached first.

### Breach semantics

The count breach condition is **strictly greater than** (`count > max_count`),
so a policy with `max_count = 10` allows exactly 10 calls before the 11th
triggers an escalation.

## Verdict states

When the enforcer runs before an action it returns one of three verdicts:

| Verdict | Meaning | Operator impact |
|---------|---------|----------------|
| `Allow` | Budget within limits | Action proceeds; nothing logged to the escalation channel |
| `Escalate(reason)` | Budget exceeded; `escalate_to` is set | Action proceeds; escalation notification dispatched asynchronously |
| `Deny(reason)` | Budget exceeded with no `escalate_to`, or store unreachable | Action is aborted; caller returns an error to the user |

When two policies for the same `scope` are both breached and one has
`escalate_to` while the other does not, **Deny beats Escalate** — the stricter
outcome wins.

## Escalation routing — approver tiers

The `escalate_to` field is a plain string. Operators encode notification tier
by convention; the enforcer round-trips it verbatim into the escalation message:

| Risk tier | Typical `escalate_to` value | Example scope |
|-----------|----------------------------|---------------|
| Tier 1 — low risk | `tier-1-reviewers@example.com` | `email_send` |
| Tier 2 — medium risk | `tier-2-reviewers@example.com` | `llm_call` |
| Tier 3 — high risk | `tier-3-reviewers@example.com` | `external_api` |

The IM gateway and webhook dispatcher route escalation notifications to the
address in the verdict's reason string. Operators can point `escalate_to` at
a Feishu group, a DingTalk webhook URL, or an email address — whatever their
downstream routing handles.

## Defining a policy

### REST API

```
POST /v1/hotl/policies
Content-Type: application/json
# Authorization: Basic ... (omit when no credential is configured)
```

**Body:**

```json
{
  "scope": "llm_call",
  "window_seconds": 3600,
  "max_count": 100,
  "max_usd": 5.00,
  "escalate_to": "tier-2-reviewers@example.com"
}
```

**Response — 201 Created:**

```json
{
  "id": "7c7b2b3a-...",
  "scope": "llm_call",
  "window_seconds": 3600,
  "max_count": 100,
  "max_usd": 5.00,
  "escalate_to": "tier-2-reviewers@example.com"
}
```

### List policies

```
GET /v1/hotl/policies[?scope=llm_call]
```

Returns all active policies, optionally filtered by scope.
Returns HTTP 503 when the HotL store is not wired (feature not enabled).

### Delete a policy

```
DELETE /v1/hotl/policies/<policy-id>
```

Returns 204 on success, 404 if the id is unknown. Policy changes take effect
on the next `check` call — there is no cache TTL to wait for.

## What happens on an Escalate verdict

1. The enforcer records the usage event in `hotl_usage_log` (optimistic insert).
2. It returns `HotlVerdict::Escalate(reason)` to the caller; the action proceeds.
3. The caller logs the escalation and dispatches an async notification to the
   address embedded in `reason` (IM gateway or email sink, depending on the
   address format and what the owner has configured).
4. An operator reviews the notification and either acknowledges the breach or
   tightens the policy via the admin UI or REST API.

There is currently no acknowledgement endpoint; operator ack is tracked outside
the system (e.g. a Feishu thread reply). A formal ack workflow is planned for
a future milestone.

## Fail-closed behaviour

If the SQLite store backing `HotlPolicyStore` is unreachable when the
enforcer runs, it returns `Deny` — the system prefers refusing one LLM call
over allowing unbounded spend when the budget ledger is down. This fail-closed
contract is validated by the `eval_under_threshold_all_allow` / `fail_closed`
eval scenarios.

## Policy hot-update

Policies take effect immediately. If you delete a policy and create a tighter
replacement, the very next enforcer check uses the new threshold — no restart
or cache flush required. The eval `eval_policy_hot_update_tighter_threshold`
covers this contract.

## Validation rules

| Rule | HTTP status on violation |
|------|--------------------------|
| `window_seconds` must be > 0 | 400 |
| At least one of `max_count` or `max_usd` must be set | 400 |
| `max_count` must be > 0 if set | 400 |
| `max_usd` must be >= 0 if set | 400 |
| Store not wired | 503 |

## Scope isolation

Policies and usage counters are scoped per `scope`. One scope exhausting its
budget has no effect on another scope's counter. This is validated by the
`eval_multi_tenant_isolation` scenario.

## Known limitations

- Email-send and webhook-invoke action sites are not yet wired; only `llm_call`
  is enforced in v1.2.3.
- There is no operator acknowledgement endpoint; manual workflow required.
- The `escalate_to` field is a freeform string — the platform does not validate
  that the address is a reachable IM channel or email inbox at create time.
- There is no dashboard for viewing current usage against a policy window;
  use the `hotl_usage_log` table directly in the interim.
