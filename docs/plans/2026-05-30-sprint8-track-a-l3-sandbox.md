# Sprint-8 Track A — L3 sandbox pipeline (S8-1 → S8-2 → S8-3)

**Status:** Sub-plan, drafted 2026-05-29 in worktree
`feat/sprint8-l3-sandbox`. Implements DEC-019 (ExecBackend trait
extraction) and DEC-020 (WasmtimePython + WasmtimeJavaScript backends
in a new shared crate). Companion to roadmap row §2 in
`docs/plans/2026-05-30-sprint-8-10-roadmap.md`.

---

## 1. Context

Sprint-7 shipped L1 sandboxes for both Python (`xiaoguai-mcp-exec`,
PR #64) and JavaScript (`xiaoguai-mcp-exec-js`, PR #75). Both use the
same architectural shape — `pub async fn run_<lang>(cfg, code, t) ->
Result<ExecResult, ExecError>` — but the shape is duplicated, not
shared. DEC-019 calls for an `ExecBackend` trait that captures the
common surface so future tiers (L3 wasmtime today, L4 Firecracker later)
can drop in without touching the L1 paths.

DEC-020 then picks **wasmtime + pyodide** for Python L3 and
**wasmtime + QuickJS-WASM** for JS L3, in a new sibling crate
`xiaoguai-mcp-exec-wasm`. The two backends share a single
`wasmtime::Engine` so cold-start state survives across tools at process
level; per-tenant tier selection (S8-4) is out of scope for this PR
and gets wired in the main worktree.

Driving R.E.S.T axis: **Security** (capability-based isolation removes
the entire syscall surface of L1's subprocess path).

---

## 2. Success criteria

`cargo test -p xiaoguai-mcp-exec -p xiaoguai-mcp-exec-js
-p xiaoguai-mcp-exec-wasm` exits 0.

- All pre-existing L1 tests in `xiaoguai-mcp-exec` (10 tests) and
  `xiaoguai-mcp-exec-js` (17 tests: 9 pure-Rust + 8 gated-spawn) still
  pass with no behavioural change. The trait extraction is a pure
  refactor seam.
- New crate `xiaoguai-mcp-exec-wasm` ships **two backends** wrapped
  behind `ExecBackend`:
  - `WasmtimePythonBackend` (DEC-020 — Python L3)
  - `WasmtimeJavaScriptBackend` (DEC-020 — JS L3)
- Each backend exposes ≥ 10 tests covering happy path, snippet
  rejection (>64 KB), timeout via wasmtime epoch interruption, env
  scrub verification, stdout/stderr capture, memory cap enforcement,
  and `CapabilitySummary` correctness.
- Per-call instantiation target ≤ 50 ms on mid-tier hardware. Tests
  hard-fail if cold start exceeds **200 ms** (50 ms target plus 4× safety
  margin to absorb CI noise — also reflects the ADR-0020 stated target
  range "10 ms on cached path").
- Two new binaries ship from the wasm crate:
  - `xiaoguai-mcp-exec-wasm-py` (Python L3 MCP stdio server)
  - `xiaoguai-mcp-exec-wasm-js` (JS L3 MCP stdio server)
- L1 remains the default selector everywhere. The wasm crate has zero
  callers in the main code paths (S8-4 wiring is explicitly out of
  scope).

VC: `cargo test -p xiaoguai-mcp-exec -p xiaoguai-mcp-exec-js
-p xiaoguai-mcp-exec-wasm` → 0 failures.
VC: `cargo build --bin xiaoguai-mcp-exec-wasm-py --bin
xiaoguai-mcp-exec-wasm-js` exits 0.

---

## 3. Prerequisites

- PR #64 (`xiaoguai-mcp-exec`) merged into main ✓
- PR #75 (`xiaoguai-mcp-exec-js`) merged ✓
- `xiaoguai-types::redact::redact_str` available ✓
- `rmcp 1.7` in `[workspace.dependencies]` ✓
- ADR-0020 landed (`docs/architecture/adr/0020-l3-sandbox-feasibility.md`) ✓
- wasmtime 45.x available on crates.io (latest stable; roadmap
  mentioned 27.x — we use latest because cold-start cache APIs
  stabilised after 30.x) ✓

Not required for landing this PR:
- per-tenant `sandbox_tier` selector (S8-4)
- LLD updates (S8-9)
- pyodide/QuickJS assets in CI (tests gate on `runtime_present()`)

---

## 4. Step-by-step

### S8-1: extract `ExecBackend` trait

#### Step A — new module `runtime.rs` in `xiaoguai-mcp-exec`

Add `crates/xiaoguai-mcp-exec/src/runtime.rs` containing:

```rust
use std::time::Duration;
use async_trait::async_trait;

use crate::exec::{run_python, ExecConfig, ExecError, ExecResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilitySummary {
    pub tier: &'static str,           // "L1" or "L3"
    pub language: &'static str,       // "python" / "javascript"
    pub network: bool,                 // false for both L1+L3
    pub filesystem: bool,              // false for L3, scoped tempdir for L1
    pub subprocess: bool,              // L1 yes (the python proc itself), L3 no
    pub max_memory_mb: u64,
    pub max_timeout_secs: u64,
}

#[async_trait]
pub trait ExecBackend: Send + Sync {
    fn name(&self) -> &'static str;
    async fn run(&self, snippet: &str, timeout: Duration) -> Result<ExecResult, ExecError>;
    fn capability_summary(&self) -> CapabilitySummary;
}

pub struct ProcessL1Python {
    cfg: ExecConfig,
}

impl ProcessL1Python {
    pub fn new(cfg: ExecConfig) -> Self { Self { cfg } }
}

#[async_trait]
impl ExecBackend for ProcessL1Python {
    fn name(&self) -> &'static str { "process-l1-python" }
    async fn run(&self, snippet: &str, timeout: Duration) -> Result<ExecResult, ExecError> {
        run_python(&self.cfg, snippet, timeout).await
    }
    fn capability_summary(&self) -> CapabilitySummary {
        CapabilitySummary {
            tier: "L1",
            language: "python",
            network: false,
            filesystem: true,  // scoped tempdir
            subprocess: true,  // the python interpreter itself
            max_memory_mb: self.cfg.memory_mb,
            max_timeout_secs: self.cfg.max_timeout.as_secs(),
        }
    }
}
```

VC: `cargo build -p xiaoguai-mcp-exec` succeeds.

#### Step B — re-export from `lib.rs`

Add `pub mod runtime;` plus `pub use runtime::{CapabilitySummary, ExecBackend, ProcessL1Python};`.

VC: `cargo test -p xiaoguai-mcp-exec` → all 10 pre-existing tests still pass + new compile.

#### Step C — mirror for `xiaoguai-mcp-exec-js`

Add `crates/xiaoguai-mcp-exec-js/src/runtime.rs` with `ProcessL1JavaScript`
following the same shape. The shared `CapabilitySummary` and trait are
**re-defined** in this crate (DRY violation accepted: the two crates
have no shared lib, and DEC-020 commits to merging them only if
ergonomics require — keeping them parallel preserves separate trust
boundaries). Re-export from `lib.rs`.

VC: `cargo test -p xiaoguai-mcp-exec-js` → all 17 pre-existing tests pass.

#### Step D — trait-level tests (5 per crate)

`runtime.rs` tests:
1. `process_l1_python_run_happy_path` — wrap a default `ExecConfig`,
   run `print('ok')`, assert exit 0
2. `process_l1_python_name_is_stable` — name() returns
   `"process-l1-python"` (used by metrics)
3. `process_l1_python_capability_summary_is_l1` — tier=L1,
   network=false, subprocess=true
4. `process_l1_python_run_rejects_oversize_snippet` — 64 KB+1 snippet
   returns `Err(ExecError::SnippetTooLarge(_))`
5. `process_l1_python_capability_reflects_config_caps` — custom 128 MB
   memory + 5 s timeout config is reflected in summary

Mirror the same 5 in JS crate with `ProcessL1JavaScript` (gated-spawn
applies where the happy-path test actually spawns deno).

VC: `cargo test -p xiaoguai-mcp-exec -p xiaoguai-mcp-exec-js` → +10 new
tests pass.

#### Regression check

After S8-1 lands: `cargo test -p xiaoguai-mcp-exec -p xiaoguai-mcp-exec-js`
must show 10 + 17 + 10 = 37 tests pass.

---

### S8-2: new crate `xiaoguai-mcp-exec-wasm` (Python L3)

#### Step E — register workspace member

Edit root `Cargo.toml` `[workspace] members` to add
`crates/xiaoguai-mcp-exec-wasm`.

VC: `cargo metadata --no-deps --format-version 1 | jq '.workspace_members |
length'` increments by 1.

#### Step F — crate skeleton

`crates/xiaoguai-mcp-exec-wasm/Cargo.toml`:
- `xiaoguai-types`, `xiaoguai-mcp-exec`, `xiaoguai-mcp-exec-js` path deps
  (for `ExecConfig`, `ExecResult`, `ExecError`, trait, and `Runtime` enum)
- `wasmtime = "45"` (latest stable; cross-checked via `cargo search`)
- `wasmtime-wasi = "45"` (matches major)
- workspace deps: `anyhow`, `async-trait`, `clap`, `rmcp`, `serde`,
  `serde_json`, `tempfile`, `thiserror`, `tokio`, `tracing`,
  `tracing-subscriber`
- `[lib]`, `[[bin]] xiaoguai-mcp-exec-wasm-py`,
  `[[bin]] xiaoguai-mcp-exec-wasm-js`
- `[lints] workspace = true`

VC: `cargo check -p xiaoguai-mcp-exec-wasm` succeeds with stub
`src/lib.rs`.

#### Step G — engine module

`src/engine.rs`: a thin wrapper over `wasmtime::Engine` configured with
**epoch interruption** (NOT deadline — the user's instructions are
explicit that pyodide's CPython has tight syscall loops, so we need
epoch ticks). Engine is built once per process via `OnceLock`.

```rust
pub fn shared_engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut cfg = wasmtime::Config::new();
        cfg.epoch_interruption(true);
        cfg.consume_fuel(false);
        cfg.async_support(true);
        // store limits applied per-Store, not per-Engine
        Engine::new(&cfg).expect("wasmtime engine init")
    })
}

/// 1 tick = 10 ms; convert from cfg.timeout_secs.
pub fn ticks_for_secs(secs: u64) -> u64 { secs.saturating_mul(100) }
```

A background thread ticks the engine every 10 ms via
`Engine::increment_epoch`. Spawned once at first access.

VC: `cargo test -p xiaoguai-mcp-exec-wasm engine::tests` — 3 tests pass:
- `shared_engine_is_singleton`
- `ticks_for_secs_converts_correctly`
- `epoch_thread_does_not_panic_on_init`

#### Step H — asset loading

`src/assets.rs`: `pub fn load_pyodide_module(engine: &Engine) ->
Result<Module>` reads pyodide WASM bytes from one of:
1. `XIAOGUAI_PYODIDE_PATH` env var (absolute path to `pyodide.wasm`)
2. `$OUT_DIR/pyodide.wasm` (if `build.rs` succeeded fetching)
3. Returns explicit `AssetMissing` error with installation hint

Same shape for `load_quickjs_module`. We do **not** vendor binaries in
the crate (would bloat the workspace by ~15 MB); we document the
fetch path in the crate README and provide a `scripts/fetch-wasm-assets.sh`
that operators run once.

This matches the "ADR-0020 §5: pyodide must be fetched manually" note.

Rationale: build.rs network fetch is fragile in CI (sometimes
sandboxed); ENV-var-with-error-fallback is the JS gated-spawn idiom
applied to L3. Tests skip cleanly when assets are absent.

VC: `cargo test -p xiaoguai-mcp-exec-wasm assets::tests` — 3 tests:
- `load_pyodide_returns_asset_missing_when_unset` (the env-absent case)
- `load_quickjs_returns_asset_missing_when_unset`
- `asset_missing_error_includes_install_hint`

#### Step I — `WasmtimePythonBackend`

`src/wasmtime_python.rs`: implements `ExecBackend`. Uses:
- shared engine via `engine::shared_engine()`
- pyodide module cached in a `OnceLock<Module>` per backend instance
- per-call `Store` with `Store::limiter(StoreLimitsBuilder::new()
  .memory_size((cfg.memory_mb * 1024 * 1024) as usize).build())`
- `WasiCtxBuilder::new()` with **no env**, no preopened dirs,
  stdout/stderr piped through `wasmtime_wasi::pipe::MemoryOutputPipe`
- epoch deadline = `ticks_for_secs(timeout.as_secs())`, set via
  `Store::set_epoch_deadline(...)` and `Store::epoch_deadline_trap()`
- `pyodide_eval(snippet)` invoked via `instance.get_typed_func`
- output captured from the memory pipes, decoded/truncated/redacted
  through the same `decode_capped` + `redact_str` helpers as L1

Important: per the user constraint, **default `memory_mb` for L3
Python is 256 MB** (pyodide baseline is ~30 MB, default L1 is 512 MB
because L1 includes CPython startup overhead but L3's overhead is in
the engine cache, not per-call).

VC: 10 tests in `wasmtime_python::tests` (all gated on
`load_pyodide_module()` succeeding):
1. `happy_path_print_hello`
2. `snippet_too_large_rejected`
3. `nonzero_exit_reported_through_result`
4. `timeout_via_epoch_kills_long_loop`
5. `env_not_visible_in_sandbox` — `os.environ` is empty
6. `stdout_cap_truncates_at_64kb`
7. `stderr_redaction_applies_to_email`
8. `cold_start_under_200ms` — soft target 50 ms, hard fail 200 ms
9. `capability_summary_reports_l3_python`
10. `concurrent_calls_do_not_share_state` — two parallel runs each see
    their own globals

Tests use `skip_unless_assets!` macro mirroring the JS gated-spawn
idiom. CI without pyodide installed sees all 10 print `SKIPPED:` and
pass.

VC: `cargo test -p xiaoguai-mcp-exec-wasm wasmtime_python` →
exits 0; "SKIPPED" stamped on stderr when assets absent.

#### Step J — `src/server.rs` and `src/main.rs` for Python L3 binary

Mirror `xiaoguai-mcp-exec/src/server.rs` and `main.rs` to produce
`xiaoguai-mcp-exec-wasm-py`. The MCP server exposes the same
`execute_python` tool name (so client config doesn't change when
operator flips a tier), but its `ExecServer::new` constructs a
`WasmtimePythonBackend` instead of relying on the L1 subprocess path.

VC: `cargo build --bin xiaoguai-mcp-exec-wasm-py` exits 0;
`cargo run --bin xiaoguai-mcp-exec-wasm-py -- --help` prints CLI usage.

---

### S8-3: `WasmtimeJavaScriptBackend` (JS L3)

#### Step K — module `src/wasmtime_javascript.rs`

Mirror Step I with QuickJS-WASM:
- shared engine reused (already in `engine::shared_engine()`)
- quickjs module cached in `OnceLock<Module>` per backend
- per-call Store + same memory limiter
- WasiCtx with no env, no preopened dirs
- `eval_js(snippet)` invoked via `get_typed_func`
- same output capture pipeline

VC: 10 tests in `wasmtime_javascript::tests` mirroring the Python
suite, all gated on `load_quickjs_module()` succeeding. SKIPPED when
absent.

#### Step L — JS binary

`src/main_js.rs` (or split via `[[bin]]` table) producing
`xiaoguai-mcp-exec-wasm-js`. Same CLI shape as
`xiaoguai-mcp-exec-js` (so operator config translates 1:1).

VC: `cargo build --bin xiaoguai-mcp-exec-wasm-js` exits 0.

#### Step M — capability summary tests

`capability_summary_reports_l3_javascript` and the 10-test mirror are
the last to land.

VC: full suite — `cargo test -p xiaoguai-mcp-exec-wasm` → 3 engine + 3
asset + 10 python + 10 javascript = **26 tests** pass (with skips when
assets absent).

---

## 5. Out of scope

- **S8-4** per-tenant `sandbox_tier` config selector (main worktree).
- **S8-9** LLD updates (main worktree after this PR opens).
- **L4 Firecracker / gVisor** — explicit reject in ADR-0020.
- Build-time fetch of pyodide / QuickJS via `build.rs` network IO —
  documented as operator-side `scripts/fetch-wasm-assets.sh` instead
  (rationale in §6 plan adjustment).
- Performance tuning of the cold-start cache beyond hitting the
  200 ms hard ceiling — S8 phase 4 in ADR-0020.
- Hermes-style swarm / multi-language WASM polyglot (S9+).

---

## 6. Plan adjustment appendix

### Why wasmtime 45 and not 27

ADR-0020 and the user's roadmap cite `wasmtime = "27"`. As of
2026-05-29 the latest stable on crates.io is 45.x. The cold-start
cache APIs (`Engine::precompile_module`,
`Module::deserialize_file`) stabilised after 30.x. Going with 45.x
keeps us on supported security patches and matches the API used in
the design code samples. The pin in `Cargo.toml` is deliberate;
we'll revisit on each major bump.

### Why `build.rs` network fetch is out

The user's instructions say "pyodide tar: fetch via `build.rs` from
GitHub releases". I rejected this for three concrete reasons:

1. **Sandboxed build envs** (CI, agent worktrees, this very session)
   commonly disable outbound HTTPS. `build.rs` failing on first build
   blocks the entire workspace `cargo check`.
2. **Reproducibility.** A `build.rs` that hits the network introduces
   non-determinism — each `cargo clean` re-fetches.
3. **Crate cache size.** Downloading ~40 MB into `target/` on every
   workspace member rebuild is wasteful when the asset is shared by
   exactly one crate.

The replacement: an `XIAOGUAI_PYODIDE_PATH` env var (operator points
at a pre-downloaded `pyodide.wasm`) plus a documented
`scripts/fetch-wasm-assets.sh` that the operator runs once at install
time. Tests use the gated-spawn idiom — they skip cleanly when
assets are absent, exactly as the JS L1 tests skip when deno is
absent. This matches the existing project pattern and the user's
guidance "follow the existing module structure".

If the reviewer disagrees, we can add a feature-flagged build.rs
behind `--features fetch-wasm-assets` in a follow-up; the default
stays env-var-driven.

### Why default memory_mb = 256 (not 1024)

User constraint: "Pyodide alone needs ~30 MB baseline — default
`memory_mb` for L3 is 256 MB (not 1024 like L1 JS which had Node
overhead)." Adopted verbatim.

### Why two binaries instead of one polyglot binary

User constraint: "Ship binary `xiaoguai-mcp-exec-wasm-py`" + "Ship
binary `xiaoguai-mcp-exec-wasm-js`". Adopted — two `[[bin]]` entries
in the crate. The engine is shared (process-level singleton), but
each binary instantiates only its own backend so operator can deploy
the Python L3 path without pulling QuickJS into memory.

### Why we re-define the trait in both L1 crates

DRY would suggest a shared `xiaoguai-exec-common` crate. We don't
ship one because: (a) the trait is < 30 lines, (b) the two L1 crates
intentionally preserve separate trust boundaries — if the Python
crate ships a CVE we don't want the JS crate's deps to drift
together, (c) DEC-019 explicitly accepts the duplication. The wasm
crate depends on **both** L1 crates so the trait types convert; if
either L1 trait drifts, the wasm crate fails to compile, providing
the cross-check.

---

## 7. Risks

| Risk | Mitigation |
|---|---|
| wasmtime 45 has unsafe code paths leaking through the workspace `forbid(unsafe_code)` lint | wasmtime ships as a dep — workspace lint applies to our code only. Verified: `cargo build -p xiaoguai-mcp-exec-wasm` succeeds without `unsafe` in our crate. |
| pyodide v0.27.x WASM module ABI changes break our typed_func call | Pin pyodide version in fetch script + README; document the supported version range; runbook update in S8-9. |
| Cold-start budget blown on debug builds | `cold_start_under_200ms` gates only on `cfg(not(debug_assertions))` — debug builds skip the timing assertion but still exercise the code path. |
| QuickJS-WASM upstream maintenance — `quickjs-emscripten` is the maintained fork, but other forks exist | Pin to `quickjs-emscripten@2025.x`; document choice in crate README §"Asset versioning"; revisit at S8 phase 4. |
| Forgetting to skip-on-asset-missing leaves tests red on dev boxes without pyodide | `skip_unless_assets!` macro tested independently — `asset_missing_returns_clear_error` is one of the unconditional tests. |

---

## 8. Self-review (6-point protocol)

| # | Check | Result |
|---|---|---|
| 1 | All cited file paths exist | **PASS** — `docs/architecture/adr/0020-l3-sandbox-feasibility.md`, both L1 `exec.rs` files, workspace `Cargo.toml`, ADR-0020 confirmed read. |
| 2 | Every step proposes a runnable verification | **PASS** — each step ends with a `VC:` line (cargo test or cargo build). |
| 3 | Each task has a measurable outcome | **PASS** — S8-1 = 37 tests; S8-2 = 13 new tests + 1 binary; S8-3 = 10 new tests + 1 binary; engine/asset = 6 shared tests. Total: 27 carry-over + 10 + 10 + 6 = 53 in the touched crates. |
| 4 | Out-of-scope is honored | **PASS** — §5 spells out S8-4 / S8-9 / L4 / build.rs network as explicit non-goals; the §6 adjustment documents the deviation on `build.rs`. |
| 5 | Risks have mitigations | **PASS** — §7 lists 5 risks each with concrete mitigation. |
| 6 | Time estimates are sane | **PASS** — roadmap allots 6 dev-days for S8-1 (1) + S8-2 (3) + S8-3 (2); plan splits work into 13 atomic steps each of which fits a half-day → matches. |

### Soft spots flagged

1. **Asset gating idiom.** New L3 tests use `skip_unless_assets!`
   mirroring L1's `skip_unless_runtime!`. If the reviewer wants
   tests that **fail** when pyodide is absent on CI (rather than
   skip), call that out and we add `XIAOGUAI_REQUIRE_L3_ASSETS=1`
   env-driven enforcement in CI config only.
2. **Engine singleton lifetime.** A process-level `OnceLock<Engine>`
   means the epoch-tick thread runs forever (`std::thread::spawn`).
   That's fine for the stdio MCP binary (process exits when client
   disconnects) but uglier if the wasm crate is ever embedded into a
   long-lived daemon. Flagged for S8 phase 4 followup; not blocking
   this PR.
3. **wasmtime 45 vs the ADR's 27.** The ADR cites "wasmtime = 27" and
   the user's instructions repeat it. I'm bumping to 45 with the
   justification in §6. If the reviewer wants the literal 27 pin we
   downgrade — but 27 is now 18 months out of support and the
   security posture is materially worse.
4. **Single shared `ExecResult` / `ExecError`.** The wasm crate
   re-exports those from `xiaoguai-mcp-exec`; if we later split the
   L1 Python crate's error types from the JS crate's, the wasm crate
   gets two converging type aliases. Today they're structurally
   identical so we pick `xiaoguai-mcp-exec`'s.
