# ADR-0015 — HotL Allow-then-Escalate model

Date: 2026-05-26
Status: Accepted

## Context

Institutional AI deployments need a middle ground between unrestricted agent action and hard blocks. Two naive extremes fail in practice:

- **Deny-on-breach**: blocks the agent the moment a budget threshold is crossed. Aggressive thresholds produce cascading false positives during normal traffic spikes; conservative thresholds offer no protection. The human is dragged into the loop synchronously, blocking work.
- **Allow-always**: no protection; cost runaway is the #1 agent-platform churn driver after the first billing surprise (see ADR-0009 context).

The HotL (Human-on-the-Loop) model targets "set budgets, let the agent run, notify a human for async review when thresholds are breached." This is the pattern used in financial fraud detection and is increasingly the default expectation in enterprise AI procurement conversations: controlled autonomy, not supervised micro-step approval.

A secondary design question is the **breach condition semantics**: `count >= max_count` (inclusive) vs `count > max_count` (exclusive-upper-bound). The off-by-one affects whether the policy limit is "at most N calls" or "up to and including N calls."

A third question is the **tier routing type**: use a Rust enum vs a string for `escalate_to` and `scope`. Enums would give exhaustive match coverage but require a code change every time a new scope or escalation destination is added. Strings let operators configure their own scopes and email addresses without a release.

## Decision

### Allow-then-Escalate as the default verdict path

When a budget is breached and `escalate_to` is configured, the enforcer returns `HotlVerdict::Escalate` (not `Deny`). The caller logs the escalation event and **allows the action to continue**. A human reviews asynchronously.

`HotlVerdict::Deny` is reserved for two cases only:
1. `escalate_to` is `None` — the tenant has no escalation route configured.
2. The policy store (PG) is unreachable — the system fails closed rather than allowing unbounded spend when the budget ledger is unavailable.

When multiple policies apply to the same `(tenant, scope)` pair, `Deny` beats `Escalate` to preserve the strictest outcome.

```
check() →
  no policy              → Allow (unconditional)
  policy, within budget  → Allow
  policy, breach, escalate_to set   → Escalate (action continues; human notified async)
  policy, breach, no escalate_to    → Deny (action blocked)
  store unreachable                 → Deny (fail-closed)
```

### Exclusive upper-bound semantics: `count > max_count`

A policy with `max_count = 3` permits exactly 3 calls in the window before escalating. The 4th call (count becomes 4, which is `> 3`) triggers escalation. This matches the natural reading: "allow up to 3 calls." The inclusive form (`>= max_count`) would escalate on the 3rd call — "allow up to 2 calls" — which is counterintuitive for operators configuring the policy.

The enforcer inserts the event optimistically before comparing, so concurrent callers see a consistent tally.

### Tier routing as plain strings

`scope` and `escalate_to` are `TEXT` columns. Operators configure their own scope labels (`"llm_call"`, `"email_send"`, `"webhook_invoke"`) and escalation destinations (email addresses, webhook URLs, Slack channel IDs) without requiring a code release. The enforcer looks up policies by `(tenant_id, scope)` string match.

### Fail-closed when store is unreachable

`HotlEnforcer::check` catches all policy-store errors and converts them to `Ok(HotlVerdict::Deny(...))` rather than propagating the `Err`. This ensures callers always get a usable verdict and the system denies rather than allows when the budget ledger is down.

### Wired action sites (v1.2)

The enforcer is wired into the LLM call path (`xiaoguai-runtime::chat_stream`). Email send and webhook invoke sites are follow-ups tracked in `docs/plans/hotl-followups.md`.

## Consequences

**Positive:**
- Operators get maximum automation with a human backstop — the agent is not blocked mid-task by a budget threshold; it continues and the human reviews after the fact.
- Deny-on-breach is still available by leaving `escalate_to` unset; the model covers both philosophies.
- Fail-closed prevents the common failure mode of "monitoring system down → unlimited spend."
- String scopes and destinations make the policy system operator-configurable without releases.
- `count > max_count` semantics are unambiguous in policy documentation: "allow at most N."

**Negative:**
- Deferred actions can pile up: if the escalation notification is ignored, many Escalate verdicts accumulate before a human intervenes. Mitigation: the admin-ui shows unacknowledged escalation count as a dashboard badge; email/webhook escalation (follow-up) closes the loop more aggressively.
- Fail-closed means a PG outage blocks all gated actions (LLM calls, etc.). Mitigation: the in-memory fallback enforcer can be configured for degraded-mode operation; PG HA is handled at the infrastructure layer.
- String-based scopes lack compile-time exhaustiveness checking. Mitigation: the known scopes are documented in the policy CRUD API schema and validated at create time.

## Implementation

- `crates/xiaoguai-api/src/hotl/enforcer.rs` — `InMemoryHotlEnforcer`, `HotlVerdict`, fail-closed path
- `crates/xiaoguai-api/src/hotl/policy.rs` — `HotlPolicy`, `HotlPolicyStore` trait, `InMemoryHotlPolicyStore`
- `crates/xiaoguai-api/src/hotl/mod.rs` — module wiring and doc summary
- Migration: `hotl_policies` + `hotl_usage_log` tables (PG backend, companion to migration 0014)

## References

- ADR-0009 — Per-tenant cost quota + token-bomb defense (complementary: HOTL is async human oversight; ADR-0009 is hard structural caps)
- `docs/plans/hotl-followups.md` — email/webhook action sites roadmap
- Anthropic "Responsible Scaling Policy" — human-in-the-loop escalation as baseline for autonomous systems
