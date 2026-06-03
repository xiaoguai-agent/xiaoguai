# Code Execution Sandbox (`xiaoguai-mcp-exec`)

Operator guide for the Python code-execution MCP server that ships with
xiaoguai. The server lets agents run short Python snippets behind a
fresh tempdir CWD, an `ulimit -v` memory cap, a wall-clock deadline, and
a scrubbed environment. Sensitive operations are gated upstream by the
agent loop's HotL enforcer (see [Tier-2 prereq](../../PR_61)).

> **Single-user deployment (DEC-033).** Xiaoguai is one self-contained
> Rust binary (`xiaoguai serve`, systemd unit `xiaoguai-core.service`)
> with an embedded SQLite database — no Postgres, no Kubernetes, no
> tenants. Inspect state with `sqlite3 ~/.xiaoguai/data.db` (under
> systemd: `/var/lib/xiaoguai/data.db`). There is a single implicit
> **owner**. When `auth.username` / `auth.password` are set the API is
> behind HTTP Basic (`-u "$USER:$PASS"`); otherwise the gate is open.

## Architecture

```
agent ReAct loop
    │  tool_call: execute_python
    │  ──► HotlGate.check("tool_call.execute_python", amount=1)
    │            │
    │            ├─ Allow  ──►  McpClient.call_tool("execute_python", {code})
    │            │                    │
    │            │                    ▼
    │            │                xiaoguai-mcp-exec (stdio)
    │            │                    │
    │            │                    ▼
    │            │            /bin/sh -c "ulimit -v $N; python3 -I main.py"
    │            │                    │   (fresh tempdir, scrubbed env)
    │            │                    ▼
    │            │              ExecResult { exit, stdout, stderr, ... }
    │            │
    │            └─ Deny   ──►  synthetic ToolResult with reason
    │                            (no subprocess spawned, no budget burn)
    ▼
```

## When to enable

- The agent needs to do small deterministic transforms (filter CSV, parse
  JSON, regex, arithmetic) that the LLM is bad at doing token-by-token.
- You can tolerate the latency of a fresh `python3` per call (~50–200 ms).
- You've staffed a HotL escalation contact in `hotl_policies` so a
  runaway agent can't burn unbounded budget.

Do NOT enable if any of:
- The host runs untrusted user-supplied agent prompts (the supervisor
  layer is process-level, not VM-level — adversarial code can still
  thrash CPU within the budget).
- Network egress isn't denied at the container layer (the sandbox does
  NOT block outbound sockets — defense in depth lives at the deploy
  layer; see [Threat Model](#threat-model)).

## Installing

The server is a workspace binary. After `cargo install --path` (or
extracting from the published artifact):

```yaml
# /etc/xiaoguai/config.yaml — mcp_servers section
agent:
  mcp_servers:
    - id: exec-py-sandbox
      transport: stdio
      command: xiaoguai-mcp-exec
      args:
        - "--timeout-secs"
        - "30"
        - "--memory-mb"
        - "512"
      env_keys: []   # never propagate secrets
```

Run `xiaoguai mcp register ...` once to persist this; the supervisor
spawns the server on demand from then on.

## Seed a HotL policy

The CLI still carries a `--tenant-id` flag in the single-user build;
pass the literal owner id `owner`:

```sh
xiaoguai hotl policy create \
  --tenant-id owner \
  --scope tool_call.execute_python \
  --window-secs 3600 \
  --max-count 50 \
  --escalate-to "ops@example.com"
```

Tune `max-count` based on what your agents actually do. 50/hour is a
reasonable starting point for analyst-style workloads; bump to 500/hour
for ETL-heavy agents.

## Knobs

| Flag / env | Default | Effect |
|---|---|---|
| `--timeout-secs` / `XIAOGUAI_MCP_EXEC__TIMEOUT_SECS` | `30` | Hard wall-clock cap. Per-call timeouts above this are clamped. |
| `--memory-mb` / `XIAOGUAI_MCP_EXEC__MEMORY_MB` | `512` | Address-space limit passed to `ulimit -v`. |
| `--workdir-parent` / `XIAOGUAI_MCP_EXEC__WORKDIR_PARENT` | OS temp | Parent for per-call tempdirs. Point at `tmpfs` on Linux for fast cleanup. |
| `--python` / `XIAOGUAI_MCP_EXEC__PYTHON` | `python3` | Python executable. Use a venv-built python if you want preinstalled libs. |
| `--no-redact-stderr` / `XIAOGUAI_MCP_EXEC__NO_REDACT` | (off) | Skip stderr PII redaction. Only flip this for debugging — see the threat model below. |

## What is captured

Each call returns a JSON payload inside an MCP `text` content block:

```jsonc
{
  "exit_code": 0,
  "stdout": "…up to 64 KB…",
  "stderr": "…up to 64 KB, PII-redacted by default…",
  "duration_ms": 187,
  "truncated": false,
  "timed_out": false
}
```

- `exit_code` is `null` when the deadline fired.
- `timed_out: true` distinguishes a deadline-kill from a graceful
  non-zero exit.
- `truncated: true` indicates output hit the 64 KB cap. Re-run with a
  smaller scope or paginate manually.
- Snippets larger than 64 KB are rejected before spawn (the supervisor
  refuses to write the file).

## Threat model

| Threat | Mitigation in this layer | Mitigation deferred to deploy |
|---|---|---|
| Snippet exfiltrates env secrets | Env stripped to allowlist (`PATH`, `LANG`, `LC_ALL`, `LC_CTYPE`) before spawn | — |
| Snippet reads/writes outside its workdir | CWD is a fresh tempdir; `Drop` removes it on every outcome | Mount `--workdir-parent` on `tmpfs` or a quota'd filesystem |
| Snippet pegs CPU | tokio deadline kills the process | Container CPU shares for harder caps |
| Snippet pegs memory | `ulimit -v $memory_mb * 1024` (kilobytes) | — |
| Snippet exfiltrates over network | **Not blocked** — sandbox does NOT restrict sockets | k8s `NetworkPolicy` / Docker `--network none` / firewall egress rules |
| Snippet writes PII to stderr → audit chain | Stderr scrubbed via the workspace `redact_str` (email, IPv4, Bearer tokens, AWS keys) | — |
| Snippet probes for HotL bypass via repeated calls | Per-tenant `tool_call.execute_python` budget gate in agent loop | — |
| Snippet spawns daemons / detaches | `kill_on_drop` on the tokio Command — when the future is dropped on deadline, SIGKILL goes to the *child process group* via `/bin/sh -c "exec …"` — child shell exits, kernel reaps the python child | — |
| Persistent state between calls | Every call uses a fresh tempdir + fresh process; nothing persists | — |

## Known limitations

- **No output captured on timeout.** When the deadline fires, the
  process is killed before we read its pipes. `stdout` and `stderr`
  come back empty with `timed_out: true`. Workaround: snippets that
  need to publish partial progress should `flush()` to a file in the
  workdir; the next call can read it back (but won't see the same
  workdir — better: post progress to an MCP `notify` channel; future
  work).
- **macOS `ulimit -v` is best-effort.** Darwin doesn't always enforce
  virtual-memory caps the way Linux does. Use Linux container deploys
  for production.
- **JavaScript is a sibling, not part of this binary.** See
  [`mcp-exec-js-sandbox.md`](mcp-exec-js-sandbox.md) for the
  `xiaoguai-mcp-exec-js` runbook — separate trust boundary, separate
  HotL scope (`tool_call.execute_javascript`), separate binary. A
  sandbox escape in one runtime must not chain into the other.

## Smoke test

```sh
# Spawn the server attached to a terminal:
xiaoguai-mcp-exec --timeout-secs 5 --memory-mb 128 < /dev/null

# Or, integration-style via the supervisor:
xiaoguai mcp list   # find the spawned id
xiaoguai chat --prompt "Use execute_python to compute the 10th Fibonacci number"
```

> **Note (post session-5):** the bare `xiaoguai chat` CLI constructs an
> `Agent::new(...)` with no MCP tools and no HotL gate wired. It will
> reply via the LLM but cannot actually invoke `execute_python`. For an
> end-to-end demo that exercises the gate + the sandbox, see
> **End-to-end demo** below — go through the running server's
> `/v1/sessions/*` API rather than the CLI one-shot.

## End-to-end demo (post session-5)

A reproducible, asciinema-recordable demo lives in
[`docs/scripts/demo-mcp-exec.sh`](../scripts/demo-mcp-exec.sh). It
covers steps 4.5 + 4.6 of
[`docs/plans/2026-05-28-agent-mcp-exec-e2e.md`](../plans/2026-05-28-agent-mcp-exec-e2e.md):

1. Confirms the server is healthy, mcp-exec is registered, and a HotL
   policy is in place.
2. Opens a `/v1/sessions` session, sends a prompt that should result in
   `execute_python(7**7)`, asserts the reply contains `823543`.
3. Asserts a `tool.invoke` `audit_log` row was written for
   `execute_python` and that a `hotl_usage_log` row for scope
   `tool_call.execute_python` was appended.
4. Flips the HotL policy to deny, resends a similar prompt, asserts the
   deny reason propagates back to the user and that the new `audit_log`
   row's `action` is `tool.deny`.

Before running the script you need:

| Prereq | How to set up |
|---|---|
| `xiaoguai serve` running | `xiaoguai serve --config ~/.xiaoguai/config.yaml` |
| Agent identity (the implicit owner) | no tenant provisioning needed |
| mcp-exec registered | `xiaoguai mcp register --name exec-sandbox --transport stdio --command $(which xiaoguai-mcp-exec)` (omit `--tenant` — there is one implicit owner) |
| HotL policy allowing `execute_python` | see "Seed a HotL policy" above |

> **Note — `docs/scripts/demo-mcp-exec.sh` predates the SQLite pivot.**
> As shipped it still expects a Postgres `DATABASE_URL`, a `demo`
> tenant, and `x-tenant-id` headers. It must be ported to the
> single-user model (drop `DATABASE_URL`/tenant, read the SQLite store
> directly) before it will run against a DEC-033 deployment. The steps
> below describe the equivalent checks you can run by hand today.

Manual equivalent of the demo's verification:

```bash
DB=~/.xiaoguai/data.db   # /var/lib/xiaoguai/data.db under systemd

# 1. The server is registered:
sqlite3 "$DB" "SELECT count(*) FROM mcp_servers WHERE name='exec-sandbox';"

# 2. Capture the tool-invoke count before the run:
sqlite3 "$DB" "
  SELECT count(*) FROM audit_log
  WHERE action='tool.invoke' AND resource LIKE '%execute_python%';"

# 3. Drive a session that calls execute_python(7**7) via the API, then
#    re-run the count above — it should have incremented.

# 4. After flipping the HotL policy to deny and re-prompting, the most
#    recent tool-gate row is a denial (`tool.deny`):
sqlite3 "$DB" "
  SELECT action FROM audit_log
  WHERE action IN ('tool.invoke','tool.deny')
    AND resource LIKE '%execute_python%'
  ORDER BY id DESC LIMIT 1;"
# Expect 'tool.deny' on the deny path.
```

The tool-execution path records audit rows with `action` =
`tool.invoke` (allowed) or `tool.deny` (gated), with the tool name in
`resource` / `details` — the SQLite `audit_log` has no separate
`tool_name` / `result` columns. The HotL enforcer tracks usage in
`hotl_usage_log` keyed by `scope` (e.g. `tool_call.execute_python`);
inspect it with:

```bash
sqlite3 ~/.xiaoguai/data.db "
  SELECT scope, amount, escalated, occurred_at
  FROM hotl_usage_log
  WHERE scope = 'tool_call.execute_python'
  ORDER BY id DESC LIMIT 5;"
```

If the deny path does not behave as expected, see the Triage table in
[`docs/plans/2026-05-28-agent-mcp-exec-e2e.md`](../plans/2026-05-28-agent-mcp-exec-e2e.md)
§5.
