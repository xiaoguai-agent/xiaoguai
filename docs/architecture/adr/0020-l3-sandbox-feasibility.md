# ADR-0020 — L3 sandbox feasibility study (wasmtime / Firecracker / gVisor)

Date: 2026-05-29
Status: Proposed (Tier-3 target, sprint TBD)
Companion: `xiaoguai-agent-design/docs/harness-engineering.md` §14 (sandbox tiering),
PHILO-XIAOGUAI-001

## Context

`xiaoguai-mcp-exec` (PR #64, Python) and `xiaoguai-mcp-exec-js` (PR #75, JavaScript)
both implement L1 sandboxing per PHILO §14: process isolation via `ulimit` + env
scrub + tempdir + tokio deadline. L1 is fast (~3 ms cold start), cheap (no daemon),
and adequate for **trusted tenant** internal code. It is NOT adequate for **untrusted
multi-tenant** code: an adversarial snippet can still thrash CPU within its
budget, attempt fingerprinting via `/proc`, and (on the JS path) spawn other
binaries through `Deno.command` even with `--allow-none` set on the main runtime.

The session-5 handoff lists "wasmtime + pyodide sandbox upgrade" as the L3 target.
This ADR studies the three candidates (wasmtime+pyodide, Firecracker micro-VM,
gVisor user-space syscall filter) and recommends one path. **Implementation is
deferred** — this ADR is design only, with a stub-deferred PR landing the
trait + tier-selector for L1/L3 swap.

## Decision drivers

1. **Cold-start latency.** L1 today is 3 ms median; agents that fan out to 10
   tool calls per turn are sensitive to a 50 ms regression.
2. **Memory footprint per invocation.** Many short-lived sandboxes vs few
   long-lived sandboxes is a different shape.
3. **Trust model.** Hostile snippets that try to exfiltrate via DNS / time-side-
   channel / `/proc` reads must fail. L3 means "even if snippet is fully
   adversarial".
4. **Operational complexity.** Daemons, kernel modules, KVM access — each adds
   ops burden.
5. **Polyglot.** A unified L3 path that runs both Python AND JavaScript
   (current L1 binaries) is preferable to two separate L3 paths.
6. **Compliance posture.** Some compliance regimes (SOC2 CC6.7, HIPAA §164.312)
   require "demonstrate isolation between tenants" — L3 makes that case
   strongly.

## Candidates evaluated

### A. wasmtime + pyodide (and equivalent QuickJS for JS)

**Mechanism.** Compile Python to WebAssembly via pyodide; run WASM in wasmtime
with explicit capability grants. JS via QuickJS or SpiderMonkey-WASM.

**Pros:**
- Polyglot in principle (Python via pyodide, JS via QuickJS-WASM, can extend
  to Lua/Ruby/etc.)
- Cold start ~ 50–100 ms (wasmtime instance + interpreter import)
- Pure user-space, no kernel features needed
- Capability-based by design (no syscall surface to filter — the WASM module
  literally cannot call syscalls)
- Same binary works on Linux + macOS + Windows
- Rust-native (wasmtime is a Rust crate)

**Cons:**
- pyodide is ~10 MB and slow to instantiate — caching the engine helps but
  doesn't eliminate
- Limited stdlib: `subprocess`, `socket` (where allowed), `multiprocessing`
  all behave differently or are absent
- pyodide's CPython is 2–4x slower than native CPython on CPU-heavy work
- WASM memory limits are per-module; cross-module IPC requires WASI or
  custom imports

**Rust ecosystem.** `wasmtime = "27"` is mature. `pyodide` distribution
needs manual fetch (no Rust crate). `quickjs-rs` works but is unmaintained;
`spidermonkey-wasm` is newer.

### B. Firecracker micro-VM

**Mechanism.** AWS Firecracker spins up a stripped-down KVM micro-VM per
snippet. Snippet runs inside a guest kernel; sandbox is the hypervisor
boundary.

**Pros:**
- Strongest isolation of the three (KVM hypervisor)
- Polyglot trivially (it's a real Linux VM)
- Used in production by AWS Lambda, Fly.io, Modal — battle-tested
- Cold start ~ 125 ms (Firecracker's own claim) including kernel boot

**Cons:**
- Linux + KVM required — does NOT work on macOS dev machines (need nested
  virt or emulation); does NOT work on most ARM cloud VMs without explicit
  config
- Requires root or CAP_SYS_ADMIN; not container-friendly
- Each VM uses 5–20 MB resident, plus ~30 MB kernel image
- Operational footprint: needs `jailer` daemon, network plumbing, snapshot
  management
- Not a Rust crate — invoked via subprocess + REST API

### C. gVisor

**Mechanism.** Google gVisor is a user-space syscall filter — when the snippet
makes a syscall, gVisor intercepts and either implements it in user-space or
rejects it. Looks like Linux to the snippet, doesn't actually touch the host
kernel.

**Pros:**
- No KVM required; works on most Linux hosts (including ARM)
- Lower memory overhead than Firecracker (~5 MB per sandbox)
- Cold start ~ 60–80 ms
- Already used for Cloud Run, GKE Sandbox

**Cons:**
- Linux-only (no macOS / Windows)
- Performance overhead is significant on syscall-heavy workloads (10–30 %)
- Compatibility gaps: not every syscall is implemented; some Python C
  extensions panic
- Not a Rust crate — invoked via `runsc` subprocess

## Recommendation: **wasmtime + pyodide** for L3, with a stub-deferred trait

| Criterion | wasmtime | Firecracker | gVisor | Winner |
|---|---|---|---|---|
| Cold start | 50–100 ms | 125 ms | 60–80 ms | wasmtime |
| Cross-platform (Mac/Linux/Win) | ✅ | ❌ Linux only | ❌ Linux only | wasmtime |
| Polyglot | ✅ (per-language WASM module) | ✅ (Linux VM) | ✅ (Linux ABI) | tie |
| Operational complexity | Low | High | Medium | wasmtime |
| Rust-native | ✅ | ❌ | ❌ | wasmtime |
| Strongest isolation | Medium (capability model) | Highest (hypervisor) | High (syscall filter) | Firecracker |
| Per-sandbox memory | 5–20 MB | 35–50 MB | 5 MB | gVisor |
| Production track record | Modal, Cloudflare Workers | AWS Lambda, Fly | GCR, GKE | Firecracker |

**Recommendation rationale:**

We pick wasmtime+pyodide because:

1. **Cross-platform** is non-negotiable for us — developers ship on macOS, and
   our compliance posture targets bare-metal Linux *and* container deploys.
   Both Firecracker and gVisor fail this for dev parity.
2. **Operational simplicity.** wasmtime is a Rust crate; we already have
   tokio everywhere. Firecracker requires a daemon + REST API + jailer setup
   that the operator must run. gVisor is a subprocess but the syscall-filter
   model is opaque to debug.
3. **Capability model fits PHILO §15.** The policy gateway has full visibility
   into what capabilities a WASM module gets — there's no "default Linux ABI"
   to over-grant.
4. **Cold-start cache is feasible.** wasmtime supports pre-instantiated
   engines via `Engine::precompile_module` + `Module::deserialize_file`. The
   pyodide CPython image is ~10 MB and can be loaded once at boot. Per-call
   cold start drops to ~10 ms on the cached path.
5. **Stub-deferred trait pattern lets us land the structure now**, ship L3 in
   a follow-up sprint. The two existing L1 crates (`xiaoguai-mcp-exec`,
   `xiaoguai-mcp-exec-js`) each grow a `runtime: Option<Box<dyn ExecBackend>>`
   field where `ExecBackend` is the new trait. L1 is the default impl;
   `WasmtimeBackend` lands as a feature-flagged crate (`xiaoguai-mcp-exec-wasm`)
   in the follow-up.

**Rejected alternatives:**

- Firecracker — macOS dev parity fail. Revisit if we ever need
  multi-tenant cloud deploy with hostile snippets at scale.
- gVisor — Linux-only and syscall-filter is too coarse a tool to inspect.

## Consequences

**Positive:**

- Single L3 path for both Python and JavaScript (pyodide + QuickJS-WASM)
- Operational overhead stays low (no daemons added)
- Polyglot extension story (Lua, Ruby, etc. via WASM ports) becomes trivial
- Cross-platform dev parity maintained
- Compliance story strengthens ("L3 sandbox is the capability-based wasmtime
  runtime") — auditor-friendly

**Negative:**

- pyodide cold start cost is real even with cache (10 ms target vs L1's 3 ms)
- pyodide's stdlib gaps will surface in agent-authored skills that assume
  full CPython — we need explicit error messages when a forbidden import is
  used (e.g., `subprocess` not available)
- Operators running CPU-heavy workloads will see 2–4x slowdown on the L3
  path; document the L1↔L3 trade in the runbook
- Adds wasmtime as a build-time dep (currently no WASM in the build chain)

**Neutral:**

- The trait extraction in PRs #64 / #75 means L1 stays the default forever;
  operators opt into L3 per-tenant via config when they need stronger
  isolation.

## Implementation phasing (separate ADR / PR will own)

| Phase | Work | Duration | Owner |
|---|---|---|---|
| 1 | Extract `ExecBackend` trait from `xiaoguai-mcp-exec` and `xiaoguai-mcp-exec-js`; L1 stays default | 2 days | core team |
| 2 | New crate `xiaoguai-mcp-exec-wasm` with wasmtime + pyodide; `runtime: wasm` config opt-in | 1–2 wk | core team |
| 3 | QuickJS-WASM for JS path (extends the existing JS sandbox crate to use the WasmtimeBackend) | 3 days | core team |
| 4 | Performance benchmarking; cold-start cache tuning; documented numbers in PHILO §14 update | 2 days | core team |
| 5 | Compliance documentation update (SOC2 CC6.7 / HIPAA §164.312 "demonstrate isolation") | 1 day | compliance |

**Total**: ~3 weeks calendar time. Spread across 2–3 sprints.

## Open questions

1. Does pyodide ship the `numpy` / `pandas` wheels by default? Many agent
   snippets use them. If not, do we ship a custom pyodide build or document
   the gap?
2. What's the right *trigger* for L1 vs L3 selection — per-tenant
   `sandbox_tier` config? Per-tool override? HotL escalation
   ("execute_python normally L1; if HotL escalates to admin approval, run
   L3")?
3. Should `xiaoguai-mcp-exec-wasm` be a separately versioned PyPI-style
   release or coupled to the main release cadence?

## References

- PR #64 — `xiaoguai-mcp-exec` (L1 Python)
- PR #75 — `xiaoguai-mcp-exec-js` (L1 JavaScript)
- `xiaoguai-agent-design/docs/harness-engineering.md` §14 — sandbox tiering
- `docs/plans/2026-05-29-next-sprint.md` §3 T7 — sprint plan reference
- wasmtime docs: <https://docs.wasmtime.dev/>
- pyodide architecture: <https://pyodide.org/en/stable/usage/wasm-constraints.html>
- Firecracker design doc: <https://github.com/firecracker-microvm/firecracker/blob/main/docs/design.md>
- gVisor what's-new: <https://gvisor.dev/docs/architecture_guide/>
