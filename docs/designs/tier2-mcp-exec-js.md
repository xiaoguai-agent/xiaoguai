# Tier-2 — `xiaoguai-mcp-exec-js` (sandboxed JavaScript-execution MCP server)

**Status:** Implemented 2026-05-29 in PR `feat/tier2-execute-javascript-mcp`
(T6). Mirrors `xiaoguai-mcp-exec` (PR #64). Not yet wired into the agent
loop — that comes as a follow-up integration.

## Motivation

Python (PR #64) covers data wrangling and arithmetic. JavaScript covers
the workload Python is awkward for:

- JSON re-shape (the agent's native interchange format).
- Regex / string transforms (V8's regex is faster and more familiar).
- DOM/HTML normalisation if the operator drops a `parse5` or
  `node-html-parser` snippet inline (Node mode only).
- Number formatting and locale work, where the JavaScript intl APIs are
  more complete than CPython's stdlib.

The driving R.E.S.T axes are **Efficiency** (smaller snippet sizes for
JSON/regex/DOM transforms than equivalent Python or shelled-out tools)
and **Security** (separate trust boundary from Python so a sandbox
escape in one runtime cannot chain into the other).

## Design choice — runtime: Deno default, Node opt-in

Three runtimes considered:

| Runtime | Pros | Cons | Verdict |
|---------|------|------|---------|
| **Deno** | `--allow-none` denies network + FS + env at the runtime level. Single-binary install. No npm-install at startup. URL-based imports are opt-in per snippet, not transitive. | Operator must `deno install` once (not on default Linux). Newer than Node so fewer operator hands-on hours. | **Default** |
| **Node.js** | Ubiquitous on Linux. Operators already know it. Faster cold start. | No `--allow-none` equivalent — sandboxing pushed to outer layer (container `--network none`, k8s `NetworkPolicy`, RO mount). `npm install` at startup is a known supply-chain surface (we forbid it: the spawn is `node main.js` only — no `npm`). | **Opt-in** via `--runtime node` |
| Embedded `boa_engine` (pure-Rust JS) | No external process, no runtime dep. | Slow on JSON, partial ES2015, no Node API parity. | Rejected; future upgrade path. |

Rationale: ship a *real* sandbox today that's safe by default. Deno's
`--allow-none` flag means the runtime itself denies network and
filesystem access — we do not have to audit our own sandbox-escape
surface for either. Node remains supported because some operators will
refuse to install Deno; under Node mode the trust boundary is weaker
and pushed to the deploy layer (documented in the runbook).

## Crate layout

```
crates/xiaoguai-mcp-exec-js/
├── Cargo.toml
└── src/
    ├── lib.rs       ← public surface (run_javascript, ExecConfig, Runtime)
    ├── exec.rs      ← subprocess wrapper: spawn, ulimit, timeout, capture
    ├── server.rs    ← MCP server (rmcp ServerHandler + run_stdio_server)
    ├── tools.rs     ← MCP tool definition: execute_javascript
    └── main.rs      ← clap CLI entrypoint
```

No `tests/` integration crate yet — the rmcp protocol path is identical
to the one PR #64 already proved end-to-end. Coverage is via unit
modules inside each `src/*.rs`.

## Public API

```rust
// crates/xiaoguai-mcp-exec-js/src/lib.rs

pub enum Runtime { Deno, Node }

#[derive(Clone, Debug)]
pub struct ExecConfig {
    pub max_timeout: Duration,
    pub memory_mb: u64,        // default 1024 (V8 needs more headroom than CPython)
    pub workdir_parent: PathBuf,
    pub runtime: Runtime,
    pub runtime_bin: PathBuf,
    pub redact_stderr: bool,
}

pub async fn run_javascript(
    cfg: &ExecConfig,
    code: &str,
    timeout_request: Duration,
) -> Result<ExecResult, ExecError>;

pub async fn run_stdio_server(cfg: ExecConfig) -> anyhow::Result<()>;
```

## MCP tool surface

One tool, intentionally narrow:

```jsonc
{
  "name": "execute_javascript",
  "description": "[WRITE] Execute a self-contained JavaScript snippet in a fresh sandbox (no network, no persistent FS, hard memory + time caps). Default runtime is Deno with --allow-none; Node.js is opt-in via server config and pushes containment to the deploy layer. Returns stdout, stderr, exit code. Each call is a fresh process; nothing persists between calls. Sensitive in scope `tool_call.execute_javascript` — gate at the agent loop with a `HotL` policy before dispatch.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "code": { "type": "string" },
      "timeout_secs": { "type": "integer", "minimum": 1, "maximum": 60, "default": 30 }
    },
    "required": ["code"],
    "additionalProperties": false
  }
}
```

Deliberately omitted:
- **TypeScript** — Deno transparently handles `.ts` but we only expose
  `.js` to keep the trust surface small.
- **npm install at startup** — under Node mode the spawn is `node
  main.js`, no `npm` invocation. URL-based imports under Deno are
  blocked by `--allow-net` being denied.
- **Persistent sessions** — every call is fresh.
- **File upload/download** — leak vector; force the LLM to inline data.

## Subprocess wrapping

`exec.rs::run_javascript(code, ExecConfig)`:

1. `tempfile::Builder` creates `xg-exec-js-XXXXXX` under
   `workdir_parent`. The `TempDir` `Drop` impl removes it on every
   outcome.
2. Write `code` to `$tmp/main.js`.
3. Spawn `/bin/sh -c "ulimit -v $N; exec <runtime>"` where the runtime
   invocation depends on the chosen `Runtime`:
   - Deno: `exec deno run --allow-none main.js`
   - Node: `exec node --no-deprecation main.js`
4. CWD = `$tmp` (the snippet sees a clean dir).
5. Inherited env: stripped to a minimal allowlist (`PATH`, `LANG`,
   `LC_ALL`, `LC_CTYPE`). Critically, `OLLAMA_HOST`, `DATABASE_URL`,
   `XIAOGUAI_AUDIT_SIGNING_KEY` — **never** propagated.
6. `tokio::time::timeout` + `kill_on_drop(true)` on the `Command`:
   when the timeout fires, the future is dropped, the child receives
   SIGKILL, the kernel reaps it.
7. Stdout/stderr captured (cap each at 64 KB; truncate marker if hit).
8. Stderr scrubbed via `xiaoguai_types::redact::redact_str` when
   `cfg.redact_stderr` (defaults true — air-gapped posture).
9. Return `ExecResult { exit_code, stdout, stderr, duration_ms,
   truncated, timed_out }`.

## HotL integration (when wired)

When the agent calls `execute_javascript`, the ReAct loop dispatches
the tool name through the enforcer with scope
`tool_call.execute_javascript`. The enforcer's budget policy
(per-tenant rows in `hotl_policies`) limits e.g. 50 calls/hour. Deny →
tool returns an error verdict the LLM can adapt to. PG/enforcer outage
→ fail-closed.

This crate does NOT consult HotL itself; the gate is upstream in the
agent loop, same separation of concerns as PR #64.

## Tests (17 total — 9 pure-Rust + 8 gated-spawn)

The gated-spawn tests use an inline `which`-style PATH probe and skip
cleanly when the runtime is not installed. CI hosts without Deno/Node
get all 17 tests passing (8 print `SKIPPED: deno not on PATH` on
stderr); developer boxes with Deno installed get full coverage.

| Test | Snippet | Expect |
|------|---------|--------|
| `snippet_too_large_short_circuits` | 64 KB + 1 byte | `ExecError::SnippetTooLarge` |
| `decode_capped_truncates_at_cap_with_marker` | n/a (pure) | marker appended |
| `decode_capped_passthrough_below_cap` | n/a (pure) | unchanged |
| `build_command_uses_only_allowlisted_env` | n/a (pure) | ENV_ALLOWLIST guard |
| `runtime_default_bin_matches_variant` | n/a (pure) | `Deno→"deno"` |
| `runtime_from_str_accepts_canonical_forms` | n/a (pure) | `"deno"/"node"/"nodejs"` |
| `runtime_invoke_deno_uses_allow_none` | n/a (pure) | `--allow-none` in invocation |
| `runtime_invoke_node_omits_allow_flags` | n/a (pure) | no `--allow-*` for Node |
| `happy_path_captures_stdout_and_exits_zero` | `console.log('hi')` | exit 0, stdout match |
| `nonzero_exit_is_reported_through_result_not_error` | `Deno.exit(3)` | `exit_code=3` |
| `timeout_kills_long_running_snippet` | `setTimeout(...5s)` w/ 500 ms | `timed_out=true` |
| `stderr_is_redacted_when_configured` | `console.error("alice@…")` | email gone |
| `redaction_disabled_preserves_stderr` | same, no redact | email present |
| `stdout_cap_is_enforced_with_truncation_marker` | 130 KB output | `truncated=true` |
| `env_secrets_do_not_leak_into_sandbox` | parent sets `XIAOGUAI_AUDIT_SIGNING_KEY` | child sees `absent` |
| `workdir_is_fresh_per_call` | write then read | absent on call 2 |
| `tool_schema_advertises_execute_javascript_with_write_marker` | n/a (pure) | `[WRITE]` prefix |

Plus tool/server pure tests:
`execute_javascript_args_parse_with_defaults`,
`execute_javascript_args_reject_missing_code`,
`execute_javascript_args_parse_smoke`,
`timeout_request_is_clamped_to_max_in_schema`,
`server_info_advertises_crate_version`,
`list_tools_returns_exactly_execute_javascript`. (Test count 23 total
in the binary report — 17 maps to the substantive coverage set; the
extra 6 are small guards.)

## Operator workflow

```yaml
# config.yaml
agent:
  mcp_servers:
    - id: exec-js-sandbox
      transport: stdio
      command: xiaoguai-mcp-exec-js
      args: ["--runtime", "deno", "--memory-mb", "1024", "--timeout-secs", "30"]
      env_keys: []   # never propagate secrets
```

HotL policy (per tenant):

```sh
xiaoguai hotl policy create \
  --tenant-id acme \
  --scope tool_call.execute_javascript \
  --window-secs 3600 \
  --max-count 50 \
  --escalate-to "ops@acme.com"
```

## Threat model

| Threat | Mitigation (Deno) | Mitigation (Node) |
|--------|------|------|
| Snippet exfiltrates env secrets | Env stripped to allowlist + `--allow-none` denies `Deno.env.*` | Env stripped to allowlist (Node has no runtime-level deny) |
| Snippet reads/writes outside workdir | CWD = fresh tempdir; `--allow-none` denies `Deno.readFile`/`writeFile` | CWD = fresh tempdir; **operator must enforce RO mount or container-level FS isolation** |
| Snippet pegs CPU | tokio timeout + container CPU shares | same |
| Snippet pegs memory | `ulimit -v` 1024 MB default (V8 needs more than CPython) | same |
| Snippet exfiltrates over network | `--allow-net` denied → all sockets refused | **Not blocked** — `require('net')`/`fetch` work; deploy `--network none` |
| Snippet writes PII to stderr → audit chain | `redact_str` before return | same |
| Snippet probes for HotL bypass | Per-tenant `tool_call.execute_javascript` budget gate upstream | same |
| Snippet spawns daemons / detaches | `kill_on_drop` → SIGKILL on timeout | same |
| **Prototype pollution chain** (snippet mutates `Object.prototype` and result re-enters agent prompt) | Output goes through `JSON.stringify` via the result payload before write — agent gets a literal string, not a live object | same |
| **eval() + remote code load** | `--allow-net` denied; URL-based imports refused at runtime | **Not blocked** — operator's outer-layer network deny is the only mitigation |
| **npm install at startup** | n/a (Deno doesn't have npm) | **Forbidden by design** — spawn is `node main.js`, never `npm install …` |

## Out of scope (this PR)

- Wiring into `xiaoguai-agent` — separate follow-up; mirror PR #66.
- HotL scope seeding — operator command documented, not pre-applied.
- TypeScript exposure.
- Embedded `boa_engine` fallback.
- E2E MCP stdio driver script — rmcp transport path is the one PR #64
  already proved.

## Open questions

- **V8 + ulimit -v interactions on macOS Darwin** vs Linux: Darwin
  doesn't always enforce VM caps the way Linux does. Use Linux
  container deploys for production. Documented in runbook.
- **Deno permission denials emit to stderr** — useful for debugging,
  but PII redaction may garble error messages. The redactor allowlists
  `console.error` style output; an operator who needs raw permission
  denial messages can flip `--no-redact-stderr` for that lane.
