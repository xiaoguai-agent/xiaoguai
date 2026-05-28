# Tier-2 — `xiaoguai-mcp-exec` (sandboxed code-execution MCP server)

**Status:** Design draft (2026-05-28). Implementation blocked by Tier-2
prereq (PR for `feat/tier2-prereq-hotl-gate` — HotL gate wired into the
agent ReAct loop's tool-call dispatch).

## Motivation

The pi/Hermes roadmap calls for programmatic tool calling — the agent
emits a code snippet and a sandboxed runtime executes it, returning
stdout/stderr/exit. This unlocks:

- Multi-step deterministic workflows the LLM doesn't have to reason
  about token-by-token (e.g., "filter this CSV, return the 5 largest
  rows").
- Lightweight data transforms without writing a dedicated MCP server
  per task.
- A safer alternative to giving the agent shell access.

xiaoguai today has **zero sandbox primitives**
(`crates/xiaoguai-mcp/src/lib.rs` notes "Still deferred: cgroup+seccomp+
netns sandbox"). This crate is the wedge.

## Design choice (recap)

Three sandbox flavors were considered:

| Flavor | Pros | Cons | Verdict |
|--------|------|------|---------|
| `wasmtime + pyodide` | Most isolated, all in-process | Heavy dep, longer cold start, learning curve | Future upgrade path |
| `landlock + seccomp` | Kernel-enforced | Linux-only — breaks macOS dev | Rejected |
| External MCP server + `ulimit/timeout` | Matches existing arch, x-platform | Less rigorous isolation than kernel sandbox | **Chosen** |

Rationale: ship a *real* sandbox today that works on the dev box and in
prod Linux containers, instead of architecting wasmtime perfection that
ships in three months. Defense in depth comes through process limits
(`ulimit -v` memory cap, tokio timeout, tmpdir CWD), network egress
deny in container-deploy, and HotL budget gate from the prereq PR.

## Crate layout

```
crates/xiaoguai-mcp-exec/
├── Cargo.toml
├── src/
│   ├── lib.rs           ← public surface (start_server, ExecConfig)
│   ├── exec.rs          ← subprocess wrapper: spawn, ulimit, timeout, capture
│   ├── server.rs        ← MCP server using `rmcp` crate (matches existing skill servers)
│   ├── tools.rs         ← MCP tool definitions: `execute_python`
│   └── redact.rs        ← re-export of xiaoguai-types::redact for stderr scrub
├── tests/
│   ├── exec_unit.rs     ← spawn semantics, ulimit honored, timeout fires
│   └── server_e2e.rs    ← MCP transport → tool call → result roundtrip
└── README.md            ← operator setup (install python3, configure limits)
```

## Public API

```rust
// crates/xiaoguai-mcp-exec/src/lib.rs

#[derive(Clone, Debug)]
pub struct ExecConfig {
    /// Hard cap on wall-clock per call. Default 30s.
    pub timeout: Duration,
    /// Address-space limit (RSS+VM) in MB; passed to `ulimit -v`. Default 512.
    pub memory_mb: u64,
    /// Working directory parent; each call gets a fresh `mktemp -d` under this.
    pub workdir_parent: PathBuf,
    /// Path to the python3 executable. Default `python3`.
    pub python: PathBuf,
    /// When true, stderr lines are passed through PII redactor before return.
    pub redact_stderr: bool,
}

/// Start the MCP stdio server. Blocks until stdin closes.
pub async fn run_stdio_server(cfg: ExecConfig) -> Result<()>;
```

## MCP tool surface

One tool, intentionally narrow:

```jsonc
{
  "name": "execute_python",
  "description": "[WRITE] Execute a self-contained Python 3 snippet in a fresh sandbox (no network, no persistent FS, hard memory/time caps). Returns stdout, stderr, exit code. Sensitive in scope `tool_call.execute_python` — gated by HotL budget.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "code": {
        "type": "string",
        "description": "Python source to execute. Stdin is empty. Stdout is captured up to 64 KB."
      },
      "timeout_secs": {
        "type": "integer",
        "minimum": 1,
        "maximum": 60,
        "default": 30,
        "description": "Wall-clock cap. Hard-bounded by server config."
      }
    },
    "required": ["code"],
    "additionalProperties": false
  }
}
```

Deliberately omitted (future):
- `execute_javascript` — separate runtime, add after Python proves stable.
- File upload/download — leak vector; force the LLM to inline data.
- Long-lived REPL sessions — every call is fresh; no state across calls.

## Subprocess wrapping

`exec.rs::run_python(code: &str, ExecConfig) -> ExecResult`:

1. `mkdir $workdir_parent/run-$uuid` (tmpdir, removed in a `Drop` guard
   regardless of outcome).
2. Write `code` to `$tmp/main.py`.
3. Spawn:
   - On Linux: `prlimit --as=$mem_bytes -- python3 -I $tmp/main.py`.
   - On macOS (dev only): `ulimit -v $mem_kb && python3 -I $tmp/main.py`
     in a shell (sub-optimal but matches the dev environment).
   - `-I` flag: isolated mode — ignores `PYTHON*` env and user site.
4. CWD = `$tmp` (the snippet sees a clean dir).
5. Inherited env: stripped to a minimal allowlist (`PATH`, `LANG`,
   `LC_*`). Critically, `OLLAMA_HOST`, `DATABASE_URL`, audit signing
   key — **never** propagated.
6. tokio `timeout(cfg.timeout, child.wait_with_output())`. On timeout,
   `child.start_kill()` + grace + SIGKILL.
7. Stdout/stderr captured (cap each at 64 KB; truncate marker if hit).
8. Stderr scrubbed via `xiaoguai_types::redact::redact_str` when
   `cfg.redact_stderr` (defaults true — air-gapped posture).
9. Return `ExecResult { exit, stdout, stderr, duration, truncated }`.

## HotL integration (from prereq PR)

When the agent calls `execute_python`, the ReAct loop dispatches the
tool name through the enforcer with scope `tool_call.execute_python`.
The enforcer's budget policy (per-tenant rows in `hotl_policies`)
limits e.g. 50 calls/hour. Deny → tool returns an error verdict the
LLM can adapt to. PG/enforcer outage → fail-closed (same).

This crate does NOT consult HotL itself; the gate is upstream in the
agent loop. Keeps the MCP server stateless w.r.t. policy.

## Tests

### Unit (`tests/exec_unit.rs`)

| Case | Setup | Expect |
|------|-------|--------|
| happy path | `print("hi")` | stdout="hi\n", exit=0 |
| timeout | `import time; time.sleep(5)` with timeout=1 | duration≈1s, exit≠0, stderr mentions timeout |
| memory cap | snippet that allocates 2 GB list | OOMs, exit≠0 |
| stderr redaction | `import sys; print("user@example.com", file=sys.stderr)` | stderr contains `[email-redacted]` |
| no network | `import urllib.request; urllib.request.urlopen("http://1.1.1.1")` | error (container deploy) — skipped on dev |
| stdout cap | snippet prints 100 KB | stdout truncated to 64 KB, `truncated=true` |

### E2E (`tests/server_e2e.rs`)

Spawn the binary, speak MCP over stdio (via `rmcp`'s test client),
`tools/list` returns `execute_python`, `tools/call` with a trivial
snippet returns the expected `ToolCallResult`.

## Operator workflow

```yaml
# config.yaml
agent:
  mcp_servers:
    # System-wide registration — admin only
    - id: exec-py-sandbox
      transport: stdio
      command: xiaoguai-mcp-exec
      args: ["--memory-mb", "512", "--timeout-secs", "30"]
      env_keys: []   # never propagate secrets
```

HotL policy (per tenant):

```sh
xiaoguai hotl policy create \
  --tenant-id acme \
  --scope tool_call.execute_python \
  --window-secs 3600 \
  --max-count 50 \
  --escalate-to "ops@acme.com"
```

## Threat model (short)

| Threat | Mitigation |
|--------|------------|
| Snippet exfiltrates secrets via env | Env stripped to allowlist before exec |
| Snippet reads / writes outside workdir | CWD = fresh tmpdir; FS-level isolation deferred to container |
| Snippet pegs CPU | tokio timeout + container CPU shares |
| Snippet pegs memory | `ulimit -v` / `prlimit --as` |
| Snippet exfiltrates over network | Network egress deny at container layer (k8s NetworkPolicy / Docker network isolation) |
| Snippet emits PII into stderr → audit log | Stderr redacted before return |
| Snippet probes for HotL bypass via repeated calls | Per-tenant budget gate upstream |
| Snippet spawns long-lived background process | tokio timeout kills the entire process group on expiry |

## Out of scope (this PR)

- Wasm sandbox — future upgrade path; the file layout supports adding
  a second `wasmtime_backend.rs` later without redesigning the public
  API.
- JavaScript runtime — separate language, deferred.
- Persistent sessions — every call is fresh.
- Resource attribution to a session/tool_call_id — comes when the
  agent loop wires `request_id` propagation.

## Implementation order (once prereq merges)

1. Cargo workspace member registration.
2. `exec.rs` with unit tests (most of the engineering risk).
3. `server.rs` skeleton speaking MCP via `rmcp::ServerHandler`.
4. `tools.rs` defining `execute_python` and dispatching to `exec.rs`.
5. E2E test via `rmcp::transport::child_process`.
6. Operator docs + README.
7. Smoke against a live xiaoguai stack: register the server, run a
   `xiaoguai chat` that exercises the tool, verify HotL budget decrements.

## Open questions

- Default memory cap: 512 MB is conservative but Python can blow that
  on numpy/pandas. Make it config-driven (we do).
- Should the worktree be `tmpfs` on Linux for fast cleanup? Defer.
- Multi-language MCP server (one binary, two tools) vs separate
  binaries per language? Separate binaries → simpler ops, separate
  trust boundaries.
