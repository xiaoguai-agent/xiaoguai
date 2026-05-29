# JavaScript Code Execution Sandbox (`xiaoguai-mcp-exec-js`)

Operator guide for the JavaScript code-execution MCP server. Sister to
`xiaoguai-mcp-exec` (Python). **Separate trust boundary** — a sandbox
escape in one runtime must not chain into the other. Separate binary,
separate HotL scope (`tool_call.execute_javascript`), separate
container if you're serious about isolation.

## Architecture

```
agent ReAct loop
    │  tool_call: execute_javascript
    │  ──► HotlGate.check("tool_call.execute_javascript", amount=1)
    │            │
    │            ├─ Allow  ──►  McpClient.call_tool("execute_javascript", {code})
    │            │                    │
    │            │                    ▼
    │            │                xiaoguai-mcp-exec-js (stdio)
    │            │                    │
    │            │                    ▼
    │            │            /bin/sh -c "ulimit -v $N; exec deno run --allow-none main.js"
    │            │                    │            (or `node --no-deprecation main.js`)
    │            │                    │   (fresh tempdir, scrubbed env)
    │            │                    ▼
    │            │              ExecResult { exit, stdout, stderr, ... }
    │            │
    │            └─ Deny   ──►  synthetic ToolResult with reason
    │                            (no subprocess spawned, no budget burn)
    ▼
```

## When to enable

- The agent needs to do JSON reshape, regex, locale-aware number
  formatting, or HTML normalisation — things JavaScript is naturally
  better at than Python.
- You can tolerate ~100–300 ms cold-start latency per call (V8 needs
  more warm-up than CPython).
- You've staffed a HotL escalation contact in `hotl_policies`.

Do NOT enable if any of:
- You haven't installed a JavaScript runtime (`deno install` or
  `apt install nodejs`). The crate does NOT bundle one.
- You picked `--runtime node` but did not configure container-level
  network egress deny — Node has no `--allow-none` flag and will
  happily open sockets.
- The host runs untrusted user-supplied agent prompts (process-level
  isolation, not VM-level — adversarial code can still thrash CPU
  within the budget).

## Installing the runtime

### Deno (recommended)

```sh
# Linux / macOS
curl -fsSL https://deno.land/install.sh | sh
# or via package manager
brew install deno         # macOS
apt install deno          # Debian/Ubuntu when available
```

Minimum version: **Deno 2.0** (older versions have a different
permissions model that the `--allow-none` shorthand doesn't cover).

### Node.js (fallback)

```sh
apt install nodejs
# verify >= 22
node --version
```

Minimum version: **Node 22** (older LTS lines lack some console/error
hardening that the spawn template assumes).

## Configure xiaoguai

```yaml
# /etc/xiaoguai/config.yaml — mcp_servers section
agent:
  mcp_servers:
    - id: exec-js-sandbox
      transport: stdio
      command: xiaoguai-mcp-exec-js
      args:
        - "--runtime"
        - "deno"
        - "--timeout-secs"
        - "30"
        - "--memory-mb"
        - "1024"
      env_keys: []   # never propagate secrets
```

Run `xiaoguai mcp register ...` once to persist this; the supervisor
spawns the server on demand from then on.

## Seed a HotL policy

```sh
xiaoguai hotl policy create \
  --tenant-id acme \
  --scope tool_call.execute_javascript \
  --window-secs 3600 \
  --max-count 50 \
  --escalate-to "ops@acme.com"
```

Tune `max-count` based on what the tenant actually does. 50/hour is a
reasonable starting point for analyst-style workloads.

## Knobs

| Flag / env | Default | Effect |
|---|---|---|
| `--timeout-secs` / `XIAOGUAI_MCP_EXEC_JS__TIMEOUT_SECS` | `30` | Hard wall-clock cap. Per-call timeouts above this are clamped. |
| `--memory-mb` / `XIAOGUAI_MCP_EXEC_JS__MEMORY_MB` | `1024` | Address-space limit passed to `ulimit -v`. V8 reserves more than CPython — start at 1024, drop to 512 only after verifying your workload doesn't OOM. |
| `--workdir-parent` / `XIAOGUAI_MCP_EXEC_JS__WORKDIR_PARENT` | OS temp | Parent for per-call tempdirs. Point at `tmpfs` on Linux for fast cleanup. |
| `--runtime` / `XIAOGUAI_MCP_EXEC_JS__RUNTIME` | `deno` | `deno` or `node`. See "Runtime trade-offs" below. |
| `--runtime-bin` / `XIAOGUAI_MCP_EXEC_JS__RUNTIME_BIN` | `deno` or `node` | Override the executable path (useful when running a pinned binary in `/opt/`). |
| `--no-redact-stderr` / `XIAOGUAI_MCP_EXEC_JS__NO_REDACT` | (off) | Skip stderr PII redaction. Only flip for debugging — Deno permission denials and stack traces may otherwise contain user-supplied identifiers. |

## Runtime trade-offs (Deno vs Node)

| Axis | Deno (`--allow-none`) | Node (no built-in sandbox) |
|------|---|---|
| Network egress | Refused at runtime | Open by default — **operator must deploy `--network none` / k8s NetworkPolicy** |
| Filesystem access | Refused at runtime | Open by default — **operator must mount RO or use container-level isolation** |
| `process.env` / `Deno.env` | Refused at runtime | Returns the (already scrubbed) sandbox env |
| `npm install` at startup | n/a | Forbidden by the spawn template (`node main.js`, no `npm`) |
| URL-based imports | Refused at runtime (network deny) | n/a |
| Cold start | ~150–250 ms | ~80–150 ms |
| Operator familiarity | Lower | Higher |

**Recommendation:** Deno by default. Switch to Node only if (a) you
cannot install Deno on the host, and (b) you have container-level
network + FS containment that closes the open trust gap.

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
- `truncated: true` indicates output hit the 64 KB cap.
- Snippets larger than 64 KB are rejected before spawn.

## Threat model

| Threat | Mitigation in this layer | Mitigation deferred to deploy |
|---|---|---|
| Snippet exfiltrates env secrets | Env stripped to allowlist; under Deno, `Deno.env.get(...)` also refused | — |
| Snippet reads/writes outside workdir | CWD = fresh tempdir; Deno `--allow-none` denies FS APIs | Under Node: mount RO, use container FS isolation |
| Snippet pegs CPU | tokio deadline kills the process | Container CPU shares for harder caps |
| Snippet pegs memory | `ulimit -v $memory_mb * 1024` | — |
| Snippet exfiltrates over network | Deno: `--allow-net` denied. **Node: NOT blocked** | Under Node: k8s `NetworkPolicy` / Docker `--network none` / firewall egress rules |
| Snippet writes PII to stderr → audit chain | Stderr scrubbed via `redact_str` | — |
| Snippet probes for HotL bypass via repeated calls | Per-tenant `tool_call.execute_javascript` budget gate in agent loop | — |
| Snippet spawns daemons / detaches | `kill_on_drop` on the tokio Command — SIGKILL on timeout reaches the entire process group via `sh -c "exec …"` | — |
| **Prototype pollution** — snippet mutates `Object.prototype`, then result re-enters the agent's prompt | Output crosses the MCP boundary as JSON text — agent receives a literal string, not a live JS object, so the pollution doesn't survive serialisation | — |
| **eval() of remote code** | Deno: `--allow-net` denied → fetch/import refused. Node: not blocked | Under Node: container-level network deny |
| **npm install / package fetch** | Spawn template is `node main.js` (no `npm install`). Under Deno, URL imports are network operations and refused by `--allow-none` | — |
| Persistent state between calls | Every call uses a fresh tempdir + fresh process; nothing persists | — |

## Known limitations

- **No output captured on timeout.** When the deadline fires, the
  process is killed before we read its pipes. `stdout` and `stderr`
  come back empty with `timed_out: true`. Same limitation as
  `xiaoguai-mcp-exec`.
- **macOS `ulimit -v` is best-effort.** Use Linux container deploys
  for production. (Same as Python.)
- **Deno cold start is heavier than Python.** Expect 150–250 ms for a
  hello-world; numerics workloads may hit 400 ms before the snippet
  even runs.
- **Node mode pushes containment to the operator.** If you pick
  `--runtime node` without container-level network + FS isolation,
  you do NOT have a sandbox — you have a JS runtime that the agent
  can drive arbitrarily. Don't ship this configuration to production
  without doing the deploy-layer work.
- **TypeScript is not exposed.** Deno transparently handles `.ts` but
  the tool schema accepts `code: string` interpreted as JavaScript
  only — we don't want to surface the TS parser to agent input.

## Smoke test

```sh
# Spawn the server attached to a terminal (Deno):
xiaoguai-mcp-exec-js --timeout-secs 5 --memory-mb 1024 < /dev/null

# Or, integration-style via the supervisor:
xiaoguai mcp list   # find the spawned id
xiaoguai chat --prompt "Use execute_javascript to compute the 10th Fibonacci number"
```

The agent should call `execute_javascript`, see exit 0, return 55, and
the HotL counter should bump by 1 in the `hotl_usage_log` table.

## Cross-link

See `docs/runbooks/mcp-exec-sandbox.md` for the Python sibling —
identical contract, different runtime, different trust boundary.
