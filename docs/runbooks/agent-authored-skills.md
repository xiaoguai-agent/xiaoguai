# Runbook — Agent-authored skills (Tier-2 D.1)

> Status: shipped in v1.5.x (Tier-2). HotL-gated, owner-approved.
> Off by default. The agent cannot author skills until the owner opts in.
>
> **Single-owner SQLite (DEC-033).** This product runs as one implicit owner
> over embedded SQLite — there is no Postgres, no `tenant_id`, and no Casbin
> RBAC. The `tenant_settings` table keeps its name but is a single-owner
> key/value store whose primary key is the owner id `ten_local_owner`. Use
> `sqlite3` against the data file (default `~/.xiaoguai/data.db`), or the
> CLI/REST surfaces below, instead of `psql`.

---

## Threat model

The "agent-authored skills" feature lets the LLM emit a `propose_skill`
tool call at runtime. Without controls this is the §11 harness-engineering
anti-pattern — "letting the LLM author its own escapes". Three layers
of defence cover it:

1. **Off by default.** The `propose_skill` tool is unregistered unless
   the owner has `allow_skill_authoring=true` in `tenant_settings`.
2. **HotL gate (bucket `skill_author`).** Every accepted proposal
   consumes one budget unit; default policy caps at 5 proposals /
   day. A 6th draft is denied at the gate and the reason is
   fed back to the LLM (it cannot retry the same draft, but it can
   wait or revise).
3. **Owner approval.** A `pending` proposal is just a DB row. Nothing
   becomes loadable until the owner calls
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

## Enabling skill authoring (owner opt-in)

The flag lives in the single owner's `tenant_settings` row, keyed by the owner
id `ten_local_owner`. With `sqlite3` against the data file:

```bash
sqlite3 ~/.xiaoguai/data.db \
  "INSERT INTO tenant_settings (tenant_id, settings)
   VALUES ('ten_local_owner', json('{\"allow_skill_authoring\": true}'))
   ON CONFLICT(tenant_id) DO UPDATE
     SET settings = json_set(tenant_settings.settings, '\$.allow_skill_authoring', json('true'));"
```

Disable again:

```bash
sqlite3 ~/.xiaoguai/data.db \
  "UPDATE tenant_settings
      SET settings = json_set(settings, '\$.allow_skill_authoring', json('false'))
    WHERE tenant_id = 'ten_local_owner';"
```

The check is read on every `propose_skill` invocation — no restart
required.

---

## HotL budget

Seed the policy bucket via the CLI (or `POST /v1/hotl/policies`). Adjust
`--max-count` per your operational tolerance. There is no `tenant_id` — the
policy applies to the single owner:

```bash
xiaoguai hotl policy create \
  --scope skill_author \
  --window-secs 86400 \   # 1 day rolling window
  --max-count 5
```

The gate is consulted with `scope = 'skill_author'` and `amount = 1.0`
per proposal. A denial returns the gate's `reason` string to the LLM
as a synthetic tool failure.

---

## Approving / rejecting

CLI:

```bash
xiaoguai skills proposals list
xiaoguai skills proposals list --status pending

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
   `sqlite3 ~/.xiaoguai/data.db "DELETE FROM skill_proposals WHERE id = '<prop-id>';"`
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

## Access control

Single-owner (DEC-033): there is no Casbin RBAC and no roles. The
approve/reject endpoints are reached by the one owner identity. Protect them
by configuring the HTTP Basic gate (`auth.username` / `auth.password`, or the
`XIAOGUAI_AUTH__USERNAME` / `XIAOGUAI_AUTH__PASSWORD` env vars) and/or
restricting `/v1/admin/**` and `/v1/skills/proposals/**` at the network layer
(reverse proxy / firewall).

---

## References

* Plan: `docs/plans/2026-05-29-tier2-d1-agent-authored-skills.md`
* HotL gate (Tier-2 prereq): `crates/xiaoguai-agent/src/hotl_gate.rs`, PR #61
* Implementation: `crates/xiaoguai-tasks/src/skill_author.rs`
* MCP tool wrapper: `crates/xiaoguai-agent/src/skill_author_tool.rs`
* HTTP routes: `crates/xiaoguai-api/src/skill_proposals.rs`
* Migration: `crates/xiaoguai-storage/migrations/0021_skill_proposals.sql`
