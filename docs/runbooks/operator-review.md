# Operator review — HotL queue triage

Applies to: **xiaoguai v1.9.0+** (sprint-12 default-on suspension).
Companion to the [hotl-escalations user guide](../user-guide/hotl-escalations.md)
and the firefighting [hotl-escalation-stuck](./hotl-escalation-stuck.md)
runbook.

This runbook covers the steady-state operator workflow: review the
queue, decide tickets, interpret timeouts, read the audit chain.

---

## Access

xiaoguai runs as a single self-contained instance with one owner. The
HotL decision endpoint (`POST /v1/hotl/decisions`) and the Admin UI HotL
pane are reachable by the authenticated owner — there is no scope gate
or per-tenant authorization. If `auth.username` / `auth.password` are set
(env `XIAOGUAI_AUTH__USERNAME` / `XIAOGUAI_AUTH__PASSWORD`), authenticate
with HTTP Basic; if they are empty, the instance is open on localhost and
no credentials are needed.

## Review the queue

**Admin UI → HotL → Pending.** Columns:

| Column | Meaning |
|---|---|
| `escalation_id` | UUID v4; matches `hotl_pending` SSE event and audit row. |
| `scope` | `tool_call.<name>` — the gated tool. |
| `amount` | Risk weight as evaluated by `HotlEnforcer`. |
| `reason` | Free-text reason from the policy match. |
| `age` | Time since the ticket was minted. Amber at >12h, red at >20h. |
| `requester` | Conversation owner. |

Decide via inline **Approve** / **Deny** buttons. Both POST to
`/v1/hotl/decisions`. Since every authenticated request is the single
owner, `decided_by` is recorded from the `note` / request body rather
than from any token claim.

CLI equivalent (handy for scripted approvals or remote ops):

```bash
# If auth is configured, pass owner credentials with HTTP Basic;
# drop -u entirely when the instance is open on localhost.
curl -X POST "${BASE}/v1/hotl/decisions" \
  -u "${XIAOGUAI_AUTH__USERNAME}:${XIAOGUAI_AUTH__PASSWORD}" \
  -H "Content-Type: application/json" \
  -d '{"escalation_id":"<uuid>","verdict":"approve","note":"reviewed by oncall"}'
```

The agent loop wakes within tens of milliseconds (in-process registry
on the `xiaoguai-core` instance that owns the conversation).

## Interpret `verdict=timeout`

Tickets that hit the **24h default expiry** without a decision resolve
as `HotlDecision::Timeout`, which the agent treats as an implicit
Deny but audits separately. In Admin UI → HotL → History these rows
show:

- `verdict` column: `timeout` (amber pill, distinct from human `deny`).
- `decided_by`: empty (no operator note — the timer fired).
- `decided_at`: minted_at + 24h.

In the chat conversation the user sees a
`chat.hotl.timeout_annotation` UX hint: "No operator responded within
24 hours; treating as Deny."

**Action.** A non-zero timeout rate is a paging signal, not a steady
state. Check:

1. Queue staffing — operators logging in and decisions decreasing the
   pending count?
2. Notification wiring — webhook receiving `hotl_pending` SSE events?
   (Out of the box there is none; see hotl-escalations user guide.)
3. Policy tuning — is the offending scope escalating too aggressively
   for the available review bandwidth?

If timeouts are bursty (queue spike during an incident), consider
temporarily setting `agent.hotl.suspend_on_escalate: false` to fall
back to v1.8.x "log and proceed", then re-enable once queue health is
restored.

## Audit log reading

Each HotL ticket generates three signed audit rows:

| Action | When | Notable fields |
|---|---|---|
| `hotl.suspended` | Ticket minted | `escalation_id`, `scope`, `amount`, `reason` |
| `hotl.decided` | Operator decides | `escalation_id`, `verdict` (`approve`/`deny`), `decided_by` (from the decision note), `note` |
| `hotl.timeout` | Timer fires (no decision) | `escalation_id`, `verdict=timeout`, `expired_at` |

Verify chain integrity (catches tampering with operator decisions):

```bash
curl -u "${XIAOGUAI_AUTH__USERNAME}:${XIAOGUAI_AUTH__PASSWORD}" \
  "${BASE}/v1/audit?action=hotl.decided&limit=50" \
  | jq -r '.entries[] | [.id, .action, .prev_hmac[:8], .hmac[:8]] | @tsv'
```

The Admin UI Audit pane renders `<ChainBadge>` per row (ok / rotation
/ broken / head) — use the same visual cue when scanning by hand.

For full compliance export use `POST /v1/audit/exports` (sprint-7 PR
#74); the export bundles a `ChainProof` so external auditors can
re-verify offline.

## Common Prometheus signals

New metrics in v1.9.0:

- `xiaoguai_hotl_suspensions_total{verdict}` — counter; verdict is
  `approve`/`deny`/`timeout`. High `timeout` ratio = staffing or
  notification gap.
- `xiaoguai_hotl_suspended_loops_gauge` — current count of suspended
  loops (in-flight tickets). Sustained growth indicates backlog.
- `xiaoguai_hotl_suspension_duration_seconds` — histogram of
  mint→resolve latency. P95 climbing toward 24h = burying timeouts.

Wire these into your SLO dashboard; the
[hotl-escalation-stuck](./hotl-escalation-stuck.md) runbook covers the
break-glass moves when any of them go red.

## Escalation path

1. Queue depth >50 or amber tickets aging past 20h → page the on-call
   operator manager.
2. P95 suspension latency >12h sustained for 1h → engineering on-call
   (likely DecisionRegistry / SSE delivery issue).
3. Audit chain broken or rotation gap unexplained → security on-call;
   stop the instance to freeze writes and pull the export bundle.
