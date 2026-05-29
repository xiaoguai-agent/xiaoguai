# Runbook — Agent-authored skills (Tier-2 D.1)

> Status: shipped in v1.5.x (Tier-2). HotL-gated, admin-approved.
> Off by default. The agent cannot author skills until an operator
> opts the tenant in.

---

## Threat model

The "agent-authored skills" feature lets the LLM emit a `propose_skill`
tool call at runtime. Without controls this is the §11 harness-engineering
anti-pattern — "letting the LLM author its own escapes". Three layers
of defence cover it:

1. **Off by default.** The `propose_skill` tool is unregistered unless
   the tenant has `allow_skill_authoring=true` in `tenant_settings`.
2. **HotL gate (bucket `skill_author`).** Every accepted proposal
   consumes one budget unit; default policy caps at 5 proposals /
   tenant / day. A 6th draft is denied at the gate and the reason is
   fed back to the LLM (it cannot retry the same draft, but it can
   wait or revise).
3. **Admin approval.** A `pending` proposal is just a DB row. Nothing
   becomes loadable until a tenant admin calls
   `POST /v1/skills/proposals/:id/approve`, which writes the YAML
   manifest to `~/.xiaoguai/skills/` (override with
   `XIAOGUAI_SKILLS_DIR`).

Additional whitelist enforcement at proposal time:
* `tool_allowlist` must reference tools already registered in the
  agent's toolbox; unknown tool names are rejected before the gate.
* `tool_allowlist` must not contain `propose_skill` itself (recursion
  guard).
* No other top-level keys are accepted — the JSON schema sets
  `additionalProperties: false`.

---

## Enabling per tenant

```sql
INSERT INTO tenant_settings (tenant_id, settings)
VALUES ('<tenant-uuid>', '{"allow_skill_authoring": true}')
ON CONFLICT (tenant_id) DO UPDATE
   SET settings = tenant_settings.settings || EXCLUDED.settings,
       updated_at = NOW();
```

Disable again:

```sql
UPDATE tenant_settings
   SET settings = settings || '{"allow_skill_authoring": false}',
       updated_at = NOW()
 WHERE tenant_id = '<tenant-uuid>';
```

The check is read on every `propose_skill` invocation — no restart
required.

---

## HotL budget

Seed the policy bucket. Adjust `max_count` per your operational
tolerance.

```sql
INSERT INTO hotl_policies (id, tenant_id, scope, window_seconds, max_count, max_usd)
VALUES (gen_random_uuid()::text,
        '<tenant-uuid>',
        'skill_author',
        86400,   -- 1 day rolling window
        5,
        NULL);
```

The gate is consulted with `scope = 'skill_author'` and `amount = 1.0`
per proposal. A denial returns the gate's `reason` string to the LLM
as a synthetic tool failure.

---

## Approving / rejecting

CLI:

```bash
xiaoguai skills proposals list --tenant-id <id>
xiaoguai skills proposals list --tenant-id <id> --status pending

xiaoguai skills proposals approve --id <prop-id> --decided-by alice@acme
xiaoguai skills proposals reject --id <prop-id> \
    --decided-by alice@acme --reason "tool_allowlist too broad"
```

HTTP (for admin UI / scripts):

```bash
curl -X POST $API/v1/skills/proposals/<id>/approve \
     -H "content-type: application/json" \
     -d '{"decided_by": "alice@acme"}'

curl -X POST $API/v1/skills/proposals/<id>/reject \
     -H "content-type: application/json" \
     -d '{"decided_by": "alice@acme", "reason": "tool_allowlist too broad"}'
```

On approval the server writes the manifest to
`$XIAOGUAI_SKILLS_DIR/<name>-<version>.yaml`. Restart the server (or
call `xiaoguai skills reload` once it lands — tracked as a follow-up)
to make the new skill loadable in the agent.

---

## Revoking an installed skill

There is no automatic uninstall yet. To revoke:

1. Delete the YAML file: `rm ~/.xiaoguai/skills/<name>-<version>.yaml`
2. Delete the DB row:
   `DELETE FROM skill_proposals WHERE id = '<prop-id>';`
3. Restart the server.

Tracked follow-up: `xiaoguai skills proposals revoke <id>` that does
both atomically and emits `skill.revoke` to the audit log.

---

## Audit chain

Every proposal lifecycle emits these audit-log rows (HMAC-chained):

| Action | When | Details JSON |
|---|---|---|
| `skill.propose` | LLM emits a draft | `{name, version, tool_allowlist}` |
| `skill.hotl_gate` | gate consulted | `{scope, verdict: allow|deny, reason}` |
| `skill.approve` | admin approves | `{proposal_id, name, version, path}` |
| `skill.reject` | admin rejects | `{proposal_id, reason}` |

Validator rejection (unknown tool, missing field) does **not** emit
audit rows — it's a quiet drop before the chain starts. Enumeration
attacks cannot be detected from the audit log; bake a Prometheus
counter into the gate adapter if you need visibility.

---

## Schema constraints (what an agent-authored manifest may contain)

The JSON schema attached to the `propose_skill` MCP tool descriptor:

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": ["name", "description", "version", "system_prompt", "tool_allowlist"],
  "properties": {
    "name":           { "type": "string", "pattern": "^[A-Za-z0-9_-]+$", "minLength": 1 },
    "description":    { "type": "string", "minLength": 1 },
    "version":        { "type": "string", "pattern": "^[0-9]+\\.[0-9]+\\.[0-9]+(-[A-Za-z0-9.-]+)?$" },
    "system_prompt":  { "type": "string", "minLength": 1 },
    "tool_allowlist": { "type": "array",  "minItems": 1, "items": { "type": "string", "minLength": 1 } }
  }
}
```

In particular the schema **does NOT** include `mcp_server_url`,
`command`, `env`, or anything else that would let the agent declare a
new MCP server or load native code. Any such field is rejected by
serde (top-level keys are typed and `additionalProperties` is false).

---

## Casbin policy

The approve/reject endpoints expect callers to carry the
`tenant_admin` (or `system_admin`) role. Add the rule to your default
policy CSV (location depends on your `xiaoguai-auth` deployment):

```
p, tenant_admin, /v1/skills/proposals/*, approve
p, system_admin, /v1/skills/proposals/*, approve
```

If RBAC enforcement is not wired (`state.authz = None`) the endpoint
falls back to the existing `require_bearer` path; in that case
restrict access at the network layer.

---

## References

* Plan: `docs/plans/2026-05-29-tier2-d1-agent-authored-skills.md`
* HotL gate (Tier-2 prereq): `crates/xiaoguai-agent/src/hotl_gate.rs`, PR #61
* Implementation: `crates/xiaoguai-tasks/src/skill_author.rs`
* MCP tool wrapper: `crates/xiaoguai-agent/src/skill_author_tool.rs`
* HTTP routes: `crates/xiaoguai-api/src/skill_proposals.rs`
* Migration: `crates/xiaoguai-storage/migrations/0021_skill_proposals.sql`
