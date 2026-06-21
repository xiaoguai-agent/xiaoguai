# Skill Pack Runtime Loader — Design & Phasing (DEFERRED)

**Status:** DEFERRED — owner decision 2026-06-21 ("暂缓 + 写设计文档"). No code is
being written now. This document captures the *verified* current state and a
phased plan so a future sprint can pick it up without re-investigating.

**Backlog origin:** "Skill runtime loader — install only records, doesn't
execute." On investigation that one-liner conflates two separate systems and
undersells the scope by roughly an order of magnitude — hence this doc.

---

## 1. Verified current state (grep-checked 2026-06-21)

There are **two disconnected systems**. They share neither code nor data.

### A. Marketplace — `crates/xiaoguai-api/src/skills.rs` + `catalog/skill_packs.json`

- `GET /v1/skills/catalog` — **16 metadata-only entries** (slug, name,
  description, category, `knobs`, `requires`). No executable payload.
- `POST /v1/skills/install` — `install_pack` records an `installed_skill_packs`
  row. `to_installed_response` (skills.rs:148-161) **always** returns empty
  `agents` / `inbound_adapters` / `outputs` and `activation_status: "pending"`.
  Comment (skills.rs:129-131): *"runtime pack loader (post-v1.2)"*.
- This is the "install doesn't execute" the backlog meant. The catalog entries
  carry no agents/adapters to load — it is a marketplace **listing**.

### B. PackLoader — `crates/xiaoguai-core/src/packs.rs` + `packs/*/pack.yaml`

- Feature-gated behind `cfg(feature = "packs")` (deps: `serde_yaml`, `tera`,
  `serde`). **Not enabled in default builds. No live caller anywhere.**
- `PackLoader::load(pack.yaml)` parses + validates a `PackManifest`
  (`name`, `version`, `requires{xiaoguai_version, features}`, `migrations[]`,
  `watches[]`, `anomalies[]`, `agents[]`, `dashboards[]`). Validation =
  file readable, YAML valid, declared paths exist on disk.
- The local `WatchRegistry` / anomaly-registry types and `register_*` are
  **no-op stubs** ("registration calls are no-ops until the watch and anomaly
  registries land with F1 and F2").
- `packs/` holds **44 manifests** (e.g. `packs/ar-collections/pack.yaml`)
  declaring migrations + watch specs + anomaly specs + agent definitions +
  dashboards. ⚠️ They are **not schema-uniform** — e.g.
  `packs/incident-triage/pack.yaml` uses a different shape (`depends` /
  `sources` / `outputs` / `feature_flag`, with no `migrations` / `watches` /
  `anomalies`). `PackManifest` (every field `#[serde(default)]`) would *parse*
  it but silently drop those keys — see the Phase-1 risk in §4.

### What is missing to actually *execute* a pack

| Pack declares | Runtime target | Status |
|---|---|---|
| `migrations[]` (SQL) | the one embedded SQLite | **touches DEC-033**; no apply mechanism, no ledger, no rollback |
| `watches[]` (WatchSpec yaml) | `xiaoguai-watch` | crate has `WatchRunner` + `WatchEvent` but **no `WatchRegistry`** and no live driver wired into `serve` |
| `anomalies[]` (AnomalySpec yaml) | `xiaoguai-anomaly` | `AnomalyRegistry` exists, but **no scheduler poll-loop consumer** drives `observe()` |
| `agents[]` (agent yaml) | `xiaoguai-orchestrator` | an `AgentRegistry` (`registry/mod.rs:177`) + `CapabilityRouter` exist but are **test-only** — never built in the serving path — and nothing binds pack `agents[]` to them |
| `dashboards[]` | admin-ui | "not wired yet" (manifest comment) |

Plus **no activation entry point**: the `pack.yaml` comment shows
`xiaoguai pack install packs/ar-collections/`, but **that CLI does not exist**,
and the marketplace `/v1/skills/install` records a row without touching
PackLoader.

---

## 2. Why deferred

- **Scale** — multi-crate (api, core, watch, anomaly, scheduler, agent),
  spanning manifest execution + runtime drivers + lifecycle. Multi-week, not a
  backlog cleanup.
- **DEC-033 sensitivity** — per-pack SQL migrations into the single embedded
  SQLite raise schema-ownership, ordering, and uninstall-rollback questions.
  Needs an explicit decision *before* any code.
- **No runtime consumer** — even a "register watches/anomalies" slice produces
  nothing observable until a poll-loop driver runs them. Value is gated on
  building that driver too.
- **Reuse-over-build / node-not-platform** — a full pack-execution engine is a
  large new surface; worth a deliberate pass against those principles.

---

## 3. Decisions the owner must make (before any build)

1. **Activation entry point.** (a) marketplace `/v1/skills/install` triggers
   PackLoader; (b) a new `xiaoguai pack install <dir>` CLI; (c) boot-time scan
   of an enabled-packs dir. *Recommendation:* (b) a CLI + an enabled-packs
   manifest, keeping the marketplace as listing-only — least coupling, clearest
   lifecycle.
2. **Migrations into the single SQLite.** Allow per-pack migrations at all? If
   yes: namespaced tables, idempotent, recorded in a pack-migrations ledger,
   reversible on uninstall. If no: packs may only bind to existing schema.
   *Recommendation:* defer migrations to a later phase; Phase 1 packs are
   schema-free.
3. **Watch/anomaly runtime.** Appetite to wire a live, scheduler-resident
   poll-loop driver that runs registered watches/anomalies? Without it,
   registration is inert.
4. **Agent execution.** Pack `agents[]` need an execution model. A candidate
   target exists — `xiaoguai-orchestrator`'s `AgentRegistry` + `CapabilityRouter`
   (`registry/mod.rs:177`) — but it is **test-only** today (never built in the
   serving path), so this means *both* wiring that registry into `serve` *and*
   loading pack `agents[]` into it. Largest unknown — likely out of scope for
   loader v1.

---

## 4. Suggested phasing (each its own PR, TDD)

- **Phase 0 (this doc)** — capture state + decisions. ✅
- **Phase 1 — parse + validate + dry-run CLI.** `xiaoguai pack validate <dir>`:
  PackLoader parses, checks `requires.features` against the running build, and
  lists what *would* be registered. No side effects. Unblocks authoring/CI of
  the `packs/*` manifests. Low risk, zero DEC-033 exposure.
  **Risk:** the 44 `packs/*` manifests are not schema-uniform (§1) — Phase 1
  must either converge them on `PackManifest` or teach the loader the variant
  shapes, and `validate` should *reject* unknown keys rather than silently drop
  them (today every field is `#[serde(default)]`).
- **Phase 2 — anomaly/watch registration + a live driver.** Wire
  `register_anomalies` → real `AnomalyRegistry` + a scheduler-resident poll
  loop; same for watches via `WatchRunner`. This is where registration becomes
  observable. Requires decision #3.
- **Phase 3 — per-pack migrations** (only if decision #2 = yes): ledgered,
  idempotent, reversible on uninstall.
- **Phase 4 — agent execution** (decision #4): out of scope until an execution
  model exists.
- **Phase 5 — marketplace ↔ loader integration:** make `/v1/skills/install`
  drive the loader and flip `activation_status` to `active`.

---

## 5. DEC-033 guardrails (binding on every phase)

- No Postgres / Redis / external queue. Migrations (if any) go into the one
  embedded SQLite.
- Single-owner — no per-tenant pack scoping.
- Single binary — pack content ships in-repo / operator-provided dirs, not a
  network registry.

---

## 6. References (all verified 2026-06-21)

- `crates/xiaoguai-core/src/packs.rs` — the stub loader (parse + validate,
  no-op register; feature `packs`; `PackManifest` schema).
- `crates/xiaoguai-api/src/skills.rs:129-161` — marketplace install;
  `activation_status` always `pending`.
- `packs/ar-collections/pack.yaml` — reference manifest (migrations + watches +
  anomalies + agents + dashboards).
- `crates/xiaoguai-watch/src/runner.rs` (`WatchRunner`, `WatchEvent`; no
  `WatchRegistry`).
- `crates/xiaoguai-anomaly/src/registry.rs` (`AnomalyRegistry`; no live poll
  consumer found in scheduler/runtime).
- No `xiaoguai pack` CLI. `xiaoguai-orchestrator` *does* define an
  `AgentRegistry` (`registry/mod.rs:177`) + `CapabilityRouter`, but both are
  test-only — never constructed in the serving path, and no pack-`agents[]`
  binding exists (grep-verified).
