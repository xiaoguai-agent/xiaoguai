# Plan A — Agent → `xiaoguai-mcp-exec` end-to-end demo

> Companion to the session-5 handoff (`docs/HANDOFF-2026-05-28-session5.md`).
> Meta-plan: `~/.claude/plans/drifting-zooming-stroustrup.md`.

## 1. Context

PR #61 wired a per-tool `HotlGate` into the agent dispatch loop. PR #64 shipped
`xiaoguai-mcp-exec` — a stdio MCP server exposing a single `[WRITE]` tool
`execute_python` that runs Python in a tempdir+ulimit+env-scrubbed sandbox.

We have **proven each piece in isolation** (live MCP stdio driver, integration
tests for the HotL gate, in-process E2E for cache and memory). What we have
**not** proven is the full chain through the running `xiaoguai serve` API:

> user prompt → agent → tool selection → HotL gate → MCP client →
> `xiaoguai-mcp-exec` subprocess → result → `hotl_usage_log` row.

The session-5 handoff scopes this at 1–2 h. Outcome: a runbook-grade
end-to-end demo that the next operator can rerun verbatim.

**Why not use the CLI `xiaoguai chat` path?** Confirmed by reading
`crates/xiaoguai-cli/src/commands/chat.rs`: it constructs a bare
`Agent::new(backend, model)` with *no* MCP tools and *no* HotL gate. It's the
wrong harness for this demo. We must drive the running server's
`/v1/sessions/*` endpoints, because that's where `run_serve` wires both
`AppState.hotl_enforcer` and `agent_defaults.hotl_gate` (see
`crates/xiaoguai-core/src/lib.rs:337–352`).

## 2. Success criteria

The demo is **done** when **every** item below produces a yes/no answer of
"yes":

1. A `SELECT count(*) FROM mcp_servers WHERE name='exec-sandbox';` on the
   demo Postgres returns `≥ 1`.
2. A session created via `POST /v1/sessions` accepts a prompt that names
   Python work, and the agent's reply contains output that **could only** come
   from a fresh subprocess run (e.g., `print(7**7)` → `823543`).
3. `SELECT count(*) FROM audit_log WHERE action='tool.execute' AND
   tool_name='execute_python';` increases by exactly 1 per request.
4. `SELECT count(*) FROM hotl_usage_log WHERE policy_bucket='exec'
   AND outcome='Allow';` increases by exactly 1 per request.
5. The same prompt issued with the policy bucket set to **Deny** returns a
   failed `ToolResult` whose `reason` field contains the policy's deny
   reason — and `audit_log` row 3 above shows `result='denied'`.
6. An asciinema cast at `docs/asciinema/agent-mcp-exec-e2e.cast` (≤ 60 s)
   reproduces criteria 2, 3, 4, and 5 in a single recording. The cast plays
   cleanly in `agg` / `asciinema-player`.
7. `docs/runbooks/mcp-exec-sandbox.md` has a new section
   `## End-to-end demo (post-session-5)` linking to the asciinema cast and
   listing the four SQL probes above.

## 3. Prerequisites

| What | How to verify |
|---|---|
| Postgres 17 + pgvector reachable | `psql "$DATABASE_URL" -c '\dx' | grep pgvector` |
| Ollama up with at least one tool-capable model (default `qwen2.5-coder`) | `curl -s localhost:11434/api/tags | jq -r '.models[].name'` |
| `xiaoguai` and `xiaoguai-mcp-exec` on `$PATH` | `which xiaoguai xiaoguai-mcp-exec && xiaoguai --version` |
| `XIAOGUAI_AUDIT_SIGNING_KEY` exported (32+ bytes) | `printf %s "$XIAOGUAI_AUDIT_SIGNING_KEY" | wc -c` ≥ 32 |
| `~/.xiaoguai/local.yaml` config with `cache.url: ""` for the air-gap variant (optional) | `xiaoguai --config ~/.xiaoguai/local.yaml smoke` |
| `asciinema` ≥ 2.4 installed (`brew install asciinema`) | `asciinema --version` |

If any check fails, abort and fix before proceeding.

## 4. Step-by-step actions

Numbered steps; each ends with a verification command (`VC:`). If a `VC`
fails, stop and triage — don't skip.

### Step 4.1 — Boot the server

```bash
OLLAMA_HOST=http://localhost:11434 \
XIAOGUAI_AUDIT_SIGNING_KEY="$XIAOGUAI_AUDIT_SIGNING_KEY" \
xiaoguai --config ~/.xiaoguai/local.yaml serve >/tmp/xiaoguai.log 2>&1 &
echo $! > /tmp/xiaoguai.pid
```

**VC:** `curl -sf localhost:7601/healthz | jq -e '.status=="ok"'` returns 0.
Also `grep -E "hotl_enforcer|hotl_gate" /tmp/xiaoguai.log` shows both gates
were constructed (lines 337–352 of `lib.rs`).

### Step 4.2 — Provision tenant + identity

We need a tenant id so the HotL bucket lookup works (`EnforcerGate` bypasses
gating when tenant is missing — see PR #61 notes).

```bash
PSQL="psql $DATABASE_URL -At"
TENANT=$($PSQL -c "INSERT INTO tenants (slug, name) VALUES ('demo','demo') \
  ON CONFLICT (slug) DO UPDATE SET name=EXCLUDED.name RETURNING id;")
echo "TENANT=$TENANT"
```

**VC:** `$PSQL -c "SELECT id FROM tenants WHERE slug='demo';"` returns
the same UUID as `$TENANT`.

### Step 4.3 — Register the mcp-exec server (tenant-scoped)

PR #64 marks mcp-exec policy-naive — we register it per-tenant so the demo
doesn't accidentally expose it globally:

```bash
xiaoguai mcp register \
  --name exec-sandbox \
  --transport stdio \
  --command "$(which xiaoguai-mcp-exec)" \
  --tenant "$TENANT"
```

**VC:** `$PSQL -c "SELECT name, transport, command FROM mcp_servers WHERE name='exec-sandbox';"`
returns one row with `transport=stdio` and the absolute path to the binary.

### Step 4.4 — Seed an Allow HotL policy

Reference the integration-test fixture shape from
`crates/xiaoguai-agent/tests/hotl_gate.rs` so we use a known-good schema.

```bash
xiaoguai hotl policy upsert \
  --tenant "$TENANT" \
  --bucket exec \
  --tool-glob 'execute_python' \
  --verdict allow-with-budget \
  --budget-per-day 50
```

**VC:** `$PSQL -c "SELECT bucket, verdict FROM hotl_policies WHERE tenant_id='$TENANT';"`
returns `exec | allow_with_budget`.

> If `xiaoguai hotl policy upsert` flag shape differs, fall back to a direct
> `INSERT INTO hotl_policies …` using the schema in
> `crates/xiaoguai-api/migrations/`. The sub-plan executor should confirm via
> `xiaoguai hotl --help` first and adjust.

### Step 4.5 — Drive the demo (Allow path)

Open a session and send a tool-bait prompt:

```bash
SESSION=$(curl -sf -X POST localhost:7601/v1/sessions \
  -H "x-tenant-id: $TENANT" \
  -H 'content-type: application/json' \
  -d '{"model":"qwen2.5-coder"}' | jq -r '.session_id')

curl -sf -X POST "localhost:7601/v1/sessions/$SESSION/messages" \
  -H "x-tenant-id: $TENANT" \
  -H 'content-type: application/json' \
  -d '{"role":"user","content":"Use Python to print 7**7. Just call execute_python."}' \
  | tee /tmp/demo-reply.json | jq -r '.message.content'
```

**VC:**
- `jq -e '.message.content | contains("823543")' /tmp/demo-reply.json` returns 0.
- `$PSQL -c "SELECT count(*) FROM audit_log WHERE action='tool.execute' AND tool_name='execute_python';"`
  has incremented by 1 from a pre-step baseline.
- `$PSQL -c "SELECT count(*) FROM hotl_usage_log WHERE outcome='Allow' AND tool_name='execute_python';"`
  has incremented by 1.

### Step 4.6 — Drive the demo (Deny path)

Flip the policy to Deny, then resend:

```bash
xiaoguai hotl policy upsert --tenant "$TENANT" --bucket exec \
  --tool-glob 'execute_python' --verdict deny --reason "demo: deny-path test"

curl -sf -X POST "localhost:7601/v1/sessions/$SESSION/messages" \
  -H "x-tenant-id: $TENANT" \
  -H 'content-type: application/json' \
  -d '{"role":"user","content":"Use Python to print 7**8."}' \
  | tee /tmp/demo-deny.json
```

**VC:**
- `jq -e '.message.content | test("demo: deny-path test"; "i")' /tmp/demo-deny.json` returns 0
  (the deny reason is propagated back to the LLM and then to the user).
- `$PSQL -c "SELECT result FROM audit_log WHERE action='tool.execute' AND tool_name='execute_python' ORDER BY ts DESC LIMIT 1;"`
  returns `denied`.

### Step 4.7 — Capture asciinema

```bash
asciinema rec -c 'bash docs/scripts/demo-mcp-exec.sh' \
  --title 'xiaoguai agent → mcp-exec → HotL E2E' \
  docs/asciinema/agent-mcp-exec-e2e.cast
```

`docs/scripts/demo-mcp-exec.sh` is a new ~30-line script that wraps steps
4.5 + 4.6 with `sleep` pauses so the cast is readable. It exits non-zero if
any VC fails.

**VC:** `asciinema play docs/asciinema/agent-mcp-exec-e2e.cast` plays without
errors and ends at the deny-path output.

### Step 4.8 — Document

Edit `docs/runbooks/mcp-exec-sandbox.md`: add a `## End-to-end demo
(post-session-5)` section that links to the asciinema cast and lists the four
SQL probes from §2 success criteria. Reference this plan file.

**VC:** `git diff --stat docs/runbooks/mcp-exec-sandbox.md docs/asciinema/agent-mcp-exec-e2e.cast docs/scripts/demo-mcp-exec.sh`
shows exactly those three paths changed.

### Step 4.9 — Commit + PR

Single commit; PR title `docs(mcp-exec): end-to-end demo + runbook section`.
No code changes in `crates/` — this PR is docs + demo script only.

**VC:** CI green on the PR (the rust.yml job will rebuild but not produce
new artefacts; the docs.yml job will rebuild mdbook).

## 5. Risks & open questions

| Risk | Mitigation |
|---|---|
| `xiaoguai hotl policy upsert` CLI flag shape differs from what step 4.4 assumes | Run `xiaoguai hotl --help` as the first sub-step and adjust. Fallback: raw INSERT against `hotl_policies`. |
| Server's chat endpoint may be `/v1/chat/completions` or `/v1/agent/messages`, not `/v1/sessions/<id>/messages` | Pre-flight `curl localhost:7601/openapi.json | jq '.paths | keys[]' | grep -i chat` to confirm. Adjust step 4.5 accordingly. |
| `qwen2.5-coder` may not reliably call `execute_python` from a one-shot prompt | Prompt-engineer: include `Always call execute_python for any arithmetic >100`. If still flaky, fall back to `llama3.1:8b-instruct` which has better tool-use scoring. |
| The `EnforcerGate` bypasses gating when `tenant_id` is missing — easy to misconfigure and get a false-positive | The `x-tenant-id` header in steps 4.5/4.6 is mandatory. Add a "negative test" sub-step that omits the header and confirms the gate is *not* recorded. |
| asciinema cast may expose secrets (`XIAOGUAI_AUDIT_SIGNING_KEY`) if env is echoed | Use `unset` before recording, or only use the cast for the `curl` requests (not the boot). |

## 6. Rollback / abort criteria

- Step 4.1 fails → kill the PID, fix the config, retry.
- Step 4.3 succeeds but step 4.5 returns no tool call → DO NOT loosen the
  policy or hack around it. Abort and triage: either the LLM model isn't
  picking up the tool description (read `tools/list` payload from the
  agent's side), or the MCP registry row isn't being loaded at request
  time.
- Any step writes to `crates/` — abort. This plan is intentionally
  docs+demo-only.

To leave the repo clean on abort:

```bash
kill "$(cat /tmp/xiaoguai.pid)" 2>/dev/null
$PSQL -c "DELETE FROM mcp_servers WHERE name='exec-sandbox';"
$PSQL -c "DELETE FROM hotl_policies WHERE tenant_id='$TENANT' AND bucket='exec';"
rm -f /tmp/demo-*.json /tmp/xiaoguai.log /tmp/xiaoguai.pid
```

## 7. Out of scope

- Changing `xiaoguai chat` to plumb the HotL gate. (Tracked separately; the
  one-shot CLI is intentionally bare for now.)
- Changing the mcp-exec sandbox itself (timeout, memory limits, redaction
  list) — covered by `docs/designs/tier2-mcp-exec.md`.
- Adding new HotL policy schema fields. The fixture in
  `crates/xiaoguai-agent/tests/hotl_gate.rs` is sufficient.
- A second sandbox language (e.g., `execute_javascript`) — see "what didn't
  get done" table in the session-5 handoff, separate plan.
- Container image work (already shipped via `release.yml`).

## 8. References

- Session-5 handoff: `docs/HANDOFF-2026-05-28-session5.md`
- HotL gate impl: `crates/xiaoguai-agent/src/hotl_gate.rs`,
  `crates/xiaoguai-agent/src/react.rs::dispatch_tools`
- HotL bridge: `crates/xiaoguai-core/src/hotl_bridge.rs`
- Server wiring of both gates: `crates/xiaoguai-core/src/lib.rs:337–352`
- HotL gate integration tests (fixture source): `crates/xiaoguai-agent/tests/hotl_gate.rs`
- mcp-exec design: `docs/designs/tier2-mcp-exec.md`
- mcp-exec runbook: `docs/runbooks/mcp-exec-sandbox.md`
- CLI chat impl (confirmed gate-less): `crates/xiaoguai-cli/src/commands/chat.rs`
- CLI mcp register impl: `target/release/xiaoguai mcp register --help` (see prerequisites)

---

## Self-review

Run after writing. Each item is yes/no.

| # | Check | Result |
|---|---|---|
| 1 | Every cited file path under "References" exists (`git ls-files`) | **PASS** — verified before commit |
| 2 | Every `VC:` is a runnable shell command, no pseudocode | **PASS** |
| 3 | Each §2 success criterion has at least one §4 step that probes it | **PASS** — crit 1→4.3VC, 2→4.5VC, 3+4→4.5VC, 5→4.6VC, 6→4.7VC, 7→4.8VC |
| 4 | §7 out-of-scope is honored by §4 — no step modifies `crates/` | **PASS** |
| 5 | Each §5 risk has a written mitigation | **PASS** |
| 6 | Largest single step ≤ 30 min; total ≤ 1.5× top estimate (1–2 h → ≤ 3 h) | **PASS** — steps 4.1–4.4 each ≤ 10 min, 4.5/4.6 ≤ 20 min each, 4.7 ≤ 15 min, 4.8 ≤ 15 min, 4.9 ≤ 10 min |

**One soft spot**: step 4.4 assumes `xiaoguai hotl policy upsert` exists with
specific flags. The executor MUST run `xiaoguai hotl --help` as their first
real action and amend the plan if the surface differs. This is the most
likely place we hit a surprise.
