# T6 — `xiaoguai-mcp-exec-js` (sandboxed JavaScript execution MCP server)

**Status:** Sub-plan, drafted 2026-05-29 in worktree
`feat/tier2-execute-javascript-mcp`. Mirrors PR #64 (`xiaoguai-mcp-exec`).

---

## 1. Context

PR #64 shipped `xiaoguai-mcp-exec` (Python sandbox). The sprint T6 row in
the Tier-2 roadmap asks for a sibling crate exposing `execute_javascript`
under the same contract:

- fresh tempdir per call
- `ulimit -v` memory cap
- tokio wall-clock deadline → SIGKILL
- env scrubbed to a 4-key allowlist (`PATH`, `LANG`, `LC_ALL`, `LC_CTYPE`)
- snippet/output 64 KB caps
- stderr through `xiaoguai_types::redact::redact_str`

The driving R.E.S.T axes are **Efficiency** (JavaScript is the natural
language for JSON/regex/DOM shape-shifting that agents otherwise burn
tokens describing) and **Security** (a JS sandbox escape must not chain
with a Python sandbox escape — separate crate, separate binary, separate
HotL scope, separate trust boundary).

### Runtime decision: **Deno**, with Node.js as configurable fallback

| Axis | Deno | Node.js |
|------|------|---------|
| Network/FS sandbox | `--allow-none` makes it the runtime's problem | We'd have to wrap with seccomp/landlock ourselves |
| Single-binary install | yes (`deno install …`) | no — needs `npm` and a node_modules cache |
| npm transitive deps at runtime | no — URL-imports, opt-in per snippet | yes — known supply-chain surface |
| Already on most Linux hosts | no — operator installs | yes — packaged everywhere |
| Operator familiarity | newer | very mature |

**Decision:** default `RUNTIME=deno`, fallback `RUNTIME=node` via env/flag.
Deno carries the L1 trust boundary because the runtime itself denies
network and FS; we do not have to audit our own sandbox-escape surface.
Node remains supported because some operators will refuse to install Deno;
when they pick Node they accept a weaker boundary documented in the
runbook (Node has no `--allow-none`, so JS code can `require('net')` and
open sockets — we mitigate at the container layer with `--network none` /
NetworkPolicy, same advice as Python).

**Rejected alternative — embedded `boa_engine` (pure-Rust JS interpreter):**
no Node-API parity, slow on JSON workloads, partial ES2015 coverage. Future
upgrade path if we ever want in-process JS like `wasmtime + pyodide` was
floated for Python.

---

## 2. Success criteria

A reviewer running `cargo test -p xiaoguai-mcp-exec-js` on a host with no
JavaScript runtime installed sees all gated-spawn tests skip cleanly and
all pure-Rust tests pass. On a host with Deno installed (or Node when
`XIAOGUAI_MCP_EXEC_JS__RUNTIME=node`), the spawn tests exercise:

- happy path: `console.log('hi')` → exit 0, stdout `"hi\n"`
- non-zero exit: `process.exit(3)` → `exit_code=3`, `succeeded()=false`
- deadline: `setTimeout(() => {}, 5000)` with 500 ms deadline → `timed_out=true`
- snippet too large: 64 KB + 1 byte rejected before spawn
- stdout cap: 150 KB output → `truncated=true` + marker
- stderr redaction: `console.error('alice@example.com')` → stderr redacted
- env isolation: parent's `XIAOGUAI_AUDIT_SIGNING_KEY` invisible inside
- workdir freshness: two consecutive calls don't share files

Plus pure-Rust unit tests (no runtime needed):

- `snippet_too_large_short_circuits`
- `decode_capped_truncates_at_cap_with_marker`
- `build_command_uses_only_allowlisted_env`
- `tool_schema_advertises_execute_javascript_with_write_marker`
- `execute_javascript_args_parse_with_defaults`
- `execute_javascript_args_reject_missing_code`
- `server_info_advertises_crate_version`
- `execute_javascript_args_reject_negative_timeout`
- `timeout_request_is_clamped_to_max`

That's **9 pure-Rust + 8 gated-spawn = 17 tests**, matching #64's count.

VC: `cargo test -p xiaoguai-mcp-exec-js` exits 0; spawn tests print
`SKIPPED: deno not on PATH` (or `node`) when runtime absent.

---

## 3. Prerequisites

- workspace ready (PR #64 merged) ✓
- `xiaoguai-types::redact::redact_str` exported ✓ (verified in repo)
- `rmcp 1.7` already in `[workspace.dependencies]` ✓
- Toolchain `1.88` in `rust-toolchain.toml` ✓

Not required for landing:
- Deno installed on CI host (tests skip if absent)
- Wiring into the agent loop (separate follow-up, like #61 was for #64)

---

## 4. Step-by-step

### Step A — register workspace member
Edit root `Cargo.toml` `[workspace] members` to add
`crates/xiaoguai-mcp-exec-js`.

VC: `cargo check --workspace` resolves cleanly (will fail until B is in
place; verified after B).

### Step B — crate skeleton
Create `crates/xiaoguai-mcp-exec-js/Cargo.toml` mirroring #64's
`Cargo.toml`: `xiaoguai-types` path dep, `rmcp` with
`["server","transport-io"]`, `anyhow`/`async-trait`/`clap`/`serde`/
`serde_json`/`tempfile`/`thiserror`/`tokio`/`tracing` from workspace.
Add `[lib]` and `[[bin]]` stanzas. Add the `[lints]` workspace inheritance.

VC: `cargo check -p xiaoguai-mcp-exec-js` succeeds with empty
`src/lib.rs`.

### Step C — `src/exec.rs`
Port `xiaoguai-mcp-exec`'s `exec.rs` with three deltas:

1. `ExecConfig` gains a `runtime: Runtime` enum (`Deno | Node`) and the
   field formerly named `python` becomes `runtime_bin: PathBuf` (so an
   operator can override the binary path independently of the kind).
2. `build_command` writes `main.js` instead of `main.py`. Shell template
   depends on runtime:
   - Deno: `ulimit -v $N 2>/dev/null; exec deno run --allow-none main.js`
   - Node: `ulimit -v $N 2>/dev/null; exec node --no-deprecation main.js`
     (Node has no built-in sandbox; the runbook makes the operator
     responsible for outer-layer network/FS containment.)
3. Exported function is `run_javascript` (not `run_python`).

Everything else — env allowlist, `kill_on_drop(true)`, `stdin` close,
output cap, `redact_str` on stderr — is byte-for-byte the same logic.

VC: pure-Rust tests in `mod tests` compile and pass without any JS
runtime installed (`snippet_too_large_short_circuits`,
`decode_capped_truncates_at_cap_with_marker`,
`build_command_uses_only_allowlisted_env`).

### Step D — `src/tools.rs`
Mirror #64's `tools.rs`:

- `pub const EXECUTE_JAVASCRIPT: &str = "execute_javascript"`
- `execute_javascript_tool()` returns an `rmcp::model::Tool` with a
  `[WRITE]` description that mentions the runtime, the `--allow-none`
  posture under Deno, the 64 KB stdout cap, and the HotL gate scope
  `tool_call.execute_javascript`.
- `ExecuteJavascriptArgs { code: String, timeout_secs: Option<u64> }`
- `ExecuteJavascriptResultPayload { exit_code, stdout, stderr,
   duration_ms, truncated, timed_out }`
- `execute_javascript_call(cfg, args) -> (Vec<Content>, bool)` —
  identical clamp logic (default 30 s, hard max 60 s, then clamp again
  to `cfg.max_timeout`).

VC: `tool_schema_advertises_execute_javascript_with_write_marker` and
`execute_javascript_args_*` tests pass.

### Step E — `src/server.rs`
Mirror #64's `server.rs`: `ExecServer` owning `Arc<ExecConfig>`,
`ServerHandler` impl with `list_tools` returning one tool and
`call_tool` dispatching on name. `run_stdio_server(cfg)` binds
`rmcp::transport::io::stdio()`.

VC: `server_info_advertises_crate_version` passes;
`Implementation::new("xiaoguai-mcp-exec-js", env!("CARGO_PKG_VERSION"))`.

### Step F — `src/main.rs` and `src/lib.rs`
`main.rs` is clap entry mirroring #64. Knobs:

- `--timeout-secs` (`XIAOGUAI_MCP_EXEC_JS__TIMEOUT_SECS`, default 30)
- `--memory-mb` (`XIAOGUAI_MCP_EXEC_JS__MEMORY_MB`, default 512)
- `--workdir-parent` (`XIAOGUAI_MCP_EXEC_JS__WORKDIR_PARENT`)
- `--runtime` (`XIAOGUAI_MCP_EXEC_JS__RUNTIME`, default `deno`,
  values `deno|node`) — drives the `Runtime` enum
- `--runtime-bin` (`XIAOGUAI_MCP_EXEC_JS__RUNTIME_BIN`) — overrides the
  binary path; defaults derived from `--runtime`
- `--no-redact-stderr` (`XIAOGUAI_MCP_EXEC_JS__NO_REDACT`)

`lib.rs` re-exports `ExecConfig`, `ExecResult`, `ExecError`,
`Runtime`, `run_javascript`, `run_stdio_server`, `ExecServer`. Forbid
unsafe.

VC: `cargo build -p xiaoguai-mcp-exec-js` produces the binary;
`./target/debug/xiaoguai-mcp-exec-js --help` prints the knobs.

### Step G — gated spawn tests
Pattern (used in each #[tokio::test] that needs a runtime):

```rust
fn runtime_available(bin: &str) -> bool {
    which::which(bin).is_ok()  // OR std::process::Command probe
}
macro_rules! skip_unless_runtime {
    ($bin:expr) => {
        if !runtime_available($bin) {
            eprintln!("SKIPPED: {} not on PATH", $bin);
            return;
        }
    };
}
```

We avoid adding `which` as a dep — use a one-line `which-like` helper:
walk `PATH`, check `is_file()`. Keeps "no new transitive deps" rule.

Tests (lifted from #64 with JS snippets):

| Test | Snippet | Expectation |
|------|---------|-------------|
| `happy_path_*` | `console.log('hello from sandbox')` | exit 0, stdout match |
| `nonzero_exit_*` | `process.exit(3)` | `exit_code=Some(3)` |
| `timeout_kills_*` | `setTimeout(()=>console.log('nope'), 5000)` deadline 500 ms | `timed_out=true`, duration < 3 s |
| `stderr_redacted_*` | `console.error('alice@example.com')` | no email in stderr |
| `redaction_disabled_*` | same, `redact_stderr=false` | email present |
| `stdout_cap_*` | `console.log('x'.repeat(130_000))` | `truncated=true` |
| `env_secrets_do_not_leak_*` | `console.log(process.env.XIAOGUAI_AUDIT_SIGNING_KEY \|\| 'absent')` | `absent` |
| `workdir_is_fresh_*` | write file then second call checks for it | absent |

VC: with `which deno` present, all 8 spawn tests pass. With Deno absent,
all 8 print `SKIPPED` and the test returns success.

### Step H — design doc + runbook
- `docs/designs/tier2-mcp-exec-js.md` — mirror existing design doc, list
  the Deno-vs-Node decision and rejected alternatives, describe the new
  threat model entries (prototype pollution, eval/URL imports).
- `docs/runbooks/mcp-exec-js-sandbox.md` — mirror existing runbook,
  describe runtime install (`deno install` or `apt install nodejs`),
  per-runtime knob differences, threat model, and HotL seed.

VC: both docs render; cross-link from existing `mcp-exec-sandbox.md`
"Known limitations" section ("No JavaScript runtime yet. — see
mcp-exec-js-sandbox.md").

### Step I — formatting + lints
`cargo fmt --check` and `cargo clippy -p xiaoguai-mcp-exec-js
--all-targets -- -D warnings`.

VC: both exit 0.

### Step J — PR
Push branch, open PR titled `feat(tier-2): execute_javascript MCP server
(T6, Hermes parity)` against `main`. Summary covers crate skeleton,
runtime decision, tests, threat-model deltas, integration deferred.

---

## 5. Risks

| Risk | Likelihood | Impact | Mitigation |
|------|:---:|:---:|---|
| `cargo test` on CI hosts without Deno **fails** instead of skipping | medium | high | `runtime_available` probe + early return; documented "SKIPPED:" log in test header |
| Node fallback silently weaker than Deno (no `--allow-none`) | high | medium | Runbook hard-warns that Node mode pushes containment to operator's container/k8s layer |
| `--allow-none` rejects a built-in we actually need (e.g. `console.error`) | low | low | `console.*` is on stdio, not "permissions"; verified locally before checking in |
| ulimit -v doesn't work the same way for V8 as for CPython (V8 reserves a huge heap up-front) | medium | medium | Runbook recommends 1024 MB (vs 512 for Python); test default reduced to match V8 reality |
| Adding `which` as a dep violates the no-new-deps rule | low | low | Inline PATH-walking helper, ~12 lines |
| PEP 604–style ambiguity in Rust trait bounds (echo of issue #32 in CLAUDE.md) | nil | n/a | N/A — Rust crate, no Pydantic/FastMCP reflection |

---

## 6. Rollback

The crate is **not** wired into the agent loop in this PR. Rollback is
purely workspace-level:

1. `git revert <merge-commit>` — removes the crate dir, workspace member
   entry, design doc, runbook.
2. No DB migrations, no behaviour change to running agents.
3. If a downstream PR registers the binary in a config but then this
   crate is reverted, `xiaoguai mcp register` would fail at startup
   with a clear "binary not found" error — operators recover by
   removing the registration.

---

## 7. Out of scope (this PR)

- **Wiring into `xiaoguai-agent`** — separate follow-up; mirror what
  PR #66 did for `execute_python`.
- **HotL scope seeding** — operator command (`xiaoguai hotl policy
  create --scope tool_call.execute_javascript`) documented but not
  pre-applied.
- **End-to-end MCP stdio driver test** (the `/tmp/mcp-exec-driver.py`
  pattern from #64) — Rust-only test coverage is sufficient for this
  PR; the rmcp protocol path is the same one that #64 already proved.
- **Embedded `boa_engine` fallback** — future upgrade path.
- **TypeScript support** — Deno transparently handles `.ts` but we only
  expose `.js` to keep the trust surface small.
- **npm install at startup** — explicitly forbidden by the Node path
  template (`exec node main.js`, no `npm install`).

---

## 8. References

- PR #64 — `xiaoguai-mcp-exec` (parent template)
- PR #66 — agent-loop wiring pattern for #64
- PR #72 — HotL gate dispatch
- `crates/xiaoguai-mcp-exec/{Cargo.toml,src/*}` — template files
- `docs/designs/tier2-mcp-exec.md` — design template
- `docs/runbooks/mcp-exec-sandbox.md` — runbook template
- `docs/HANDOFF-2026-05-28-session5.md` — Tier-2 row in roadmap
- CLAUDE.md §"Threat model new for JavaScript" — prototype pollution
  + eval chains addressed in runbook

---

## Self-review (6-point protocol)

1. **Is the problem stated in 1–2 sentences a reviewer can repeat?**
   Yes — Hermes parity for JS, separate trust boundary from #64,
   `execute_javascript` MCP tool with the same env-scrub/ulimit/timeout/
   cap contract.

2. **Is every success criterion machine-verifiable?**
   Yes — each criterion is a named test (`cargo test -p
   xiaoguai-mcp-exec-js -- <name>`) or a shell exit code
   (`cargo fmt --check`, `cargo clippy ... -D warnings`).
   The "skips cleanly when Deno absent" criterion is verified by
   eyeballing stderr output of the test binary for `SKIPPED:` markers.

3. **Are the prerequisites real and satisfied?**
   `redact_str` verified at
   `crates/xiaoguai-types/src/redact.rs:34`. `rmcp 1.7` in
   `[workspace.dependencies]` lines 128–129. `tempfile` in workspace.

4. **Does each step have a verifiable checkpoint?**
   Yes — every step ends with `VC: ...`. Step A's VC reads "will fail
   until B"; explicit about the ordering dependency.

5. **Risks: which is the most likely to actually bite?**
   Most likely: V8 reserving a large heap up-front means `ulimit -v 512`
   fails Deno start. Mitigation reduces default test memory and
   documents bumping to 1024 MB. **Flagged to revisit during
   implementation** — if `deno run --allow-none` requires more than
   1024 MB even for hello-world, the test bumps to 2048 MB and the
   runbook reflects that as the new floor.

6. **Rejected alternative documented?**
   Yes — `boa_engine` (pure-Rust JS interpreter): documented in §1
   "Rejected alternative". Node-as-default rejected: documented in §1
   table (weaker default trust boundary; available as opt-in).

### Self-review verdict: **PASS** — proceed to implementation.

Open flag for implementation: validate `deno run --allow-none main.js`
runs under `ulimit -v 524288` (512 MB KB). If it OOMs, bump default
config memory to 1024 MB and the test cfg to 1024 as well.
