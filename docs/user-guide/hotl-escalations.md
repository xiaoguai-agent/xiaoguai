# HotL escalations — what they mean and what operators do

Applies to: **xiaoguai v1.9.0+** (sprint-12 default-on flip).
Older versions: v1.8.x logged escalations and let the call through; see
"Opt out (v1.8.x behaviour)" below if you need that for a transition
period.

---

## What is HotL?

Human-on-the-Loop (HotL) is the policy layer that decides, per
tool call, whether the agent must pause for an operator before the side
effect runs. Policies live in `hotl_policies` and are evaluated by
`HotlEnforcer` against the per-call scope (`tool_call.<name>`) and
amount (cost / risk weight).

The enforcer can return one of three verdicts:

| Verdict | Meaning | v1.9.0 behaviour |
|---|---|---|
| `Allow` | Within policy limits | Tool call dispatches immediately. |
| `Deny` | Policy violation | Tool call is rejected; the ReAct loop surfaces a `tool_error` and may replan. |
| `Escalate` | Operator review required | The agent loop **suspends**, the chat-ui shows a pending banner, and an entry appears in the admin HotL queue. |

The third case — `Escalate` — is what changed in v1.9.0.

---

## What "suspension" means

When `Escalate` fires, the loop:

1. Mints a HotL ticket (`escalation_id` UUID v4) and registers a waiter
   on the in-process `DecisionRegistry`.
2. Emits an `AgentEvent::HotlPending { escalation_id, scope, amount, reason }`.
   The SSE stream forwards this as a `hotl_pending` event — chat-ui
   renders an inline `<HotlBanner>` with Approve / Deny / Approve-and-remember
   buttons.
3. Writes an audit row (`hotl.suspended`) signed into the HMAC chain.
4. Stops driving the LLM; no further tool calls dispatch until the
   ticket resolves.

Two things can resolve a ticket:

- **Operator decision** via the chat-ui banner or
  `POST /v1/hotl/decisions { escalation_id, verdict: "approve"|"deny", note? }`.
  The decision is recorded with `decided_by` from the JWT, audited, and
  pushed into the registry — the loop wakes, dispatches (Approve) or
  rejects (Deny), and continues.
- **Timeout** — default **24 hours**. After the window elapses with no
  decision, the loop wakes with `HotlDecision::Timeout`, which it
  treats as an **implicit Deny**. The audit row carries
  `verdict=timeout` (distinct from a human Deny so triage can tell them
  apart). The chat-ui banner converts to a `chat.hotl.timeout_annotation`
  hint.

The chat session itself is not torn down — the user sees "Awaiting
operator review…" and the conversation resumes inline once the
operator decides. SSE consumers get a `hotl_resolved { escalation_id,
verdict }` event.

---

## Operator action required

Operators with the `hotl:decide` Casbin scope can:

- Watch the live HotL queue at **Admin UI → HotL → Pending** (renders
  ticket id, tenant, scope, amount, reason, age, requester).
- Approve / Deny inline. The button uses the same
  `POST /v1/hotl/decisions` endpoint as the chat-ui banner.
- Filter by tenant / scope / age. Tickets near the 24h timeout floor
  surface in amber.

There is **no email out of the box** — escalations live in the queue
and the chat-ui pending banner. Wire your own webhook against the
`hotl_pending` SSE stream or the audit chain if you need paging.

---

## What chat users see

The chat-ui keeps the user's previous message and the agent's
partial response visible. A non-modal banner appears below the active
bubble:

> **Awaiting operator review.**
> The agent paused before running `<scope>`. An operator will approve
> or deny shortly. Reason: `<reason>`.

If the operator approves, the banner clears and the agent resumes
inline (no page reload). If the operator denies, the banner reads "The
operator declined this step. The agent will replan." and the loop
continues with the Deny surfaced as a tool error.

If the timeout fires, the banner reads "No operator responded within
24 hours; treating as Deny." (the `chat.hotl.timeout_annotation` UX
hint — see [operator-review runbook](../runbooks/operator-review.md)).

---

## Opt out (v1.8.x behaviour)

If you tested on v1.8.x and need the old "log and dispatch" behaviour
during your operator-review rollout, set:

```yaml
# config.yaml
agent:
  hotl:
    suspend_on_escalate: false
```

Or via env: `XIAOGUAI_AGENT__HOTL__SUSPEND_ON_ESCALATE=false`.

With the opt-out in place, `Escalate` verdicts fold to `Allow` and emit
a `tracing::warn` line; **no operator gate runs**, **no chat-ui banner
appears**, and **no `hotl_pending` SSE event is emitted**. Audit rows
still record the escalation so you can backfill once you're ready.

The opt-out is intended as a transition tool, not a long-term posture
— policies that escalate but never gate are effectively just warnings.

---

## SSE events (reference)

For frontend / observability integrators, the chat SSE stream gains
two new event types in v1.9.0:

```json
{ "type": "hotl_pending",  "escalation_id": "…", "scope": "tool_call.deploy", "amount": 1.0, "reason": "…" }
{ "type": "hotl_resolved", "escalation_id": "…", "verdict": "approve" }
```

See `xiaoguai-agent-design/docs/api-contract.md` §2.6.3 for the full
schema and the `lld/lld-chat-ui.md` §4.3 contract for the banner
state machine.

---

## Related

- Runbook: [operator-review](../runbooks/operator-review.md) — queue
  triage, timeout interpretation, audit log reading.
- Runbook: [hotl-escalation-stuck](../runbooks/hotl-escalation-stuck.md)
  — what to do when escalations pile up or the enforcer hangs.
- API: `POST /v1/hotl/decisions` — full request/response schema in
  `api-contract.md` §2.6.2.
