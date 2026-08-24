# ADR-0024 — MSRV policy: a stable-minus-2 band (and rustc 1.94 → 1.95, wasmtime 47 → 48)

Date: 2026-08-24
Status: Accepted
Supersedes: [ADR-0023](0023-rust-toolchain-bump-194.md)

## Context

The MSRV has been raised twice by force, each time as a scramble:

| ADR | bump | trigger |
|---|---|---|
| ADR-0021 | 1.88 → 1.93 | wasmtime 38 → 45, five RUSTSEC advisories |
| ADR-0023 | 1.93 → 1.94 | wasmtime 45 → 47, RUSTSEC-2026-0222 |

ADR-0023 closed with: *"The next MSRV-raising dependency will need another
joined decision; consider moving to a rolling 'stable minus N' policy if that
recurs."*

**It recurred the same day.** Dependabot opened #499 and #500 for
wasmtime 48.0.0, which requires rustc 1.95. They were closed — correctly under
the rules as they stood, since 47.0.4 carries no advisory and a toolchain bump
was treated as an exceptional event needing its own justification.

That is the wrong default. The repo sat on 1.94 while stable was 1.98 — four
releases behind — and each in-band dependency bump had to re-argue the same
question from scratch.

## Decision

**The MSRV may move freely up to `current stable − 2`.**

- Within the band, a dependency that raises the MSRV is **routine**. Take it,
  no ADR, no debate.
- Beyond the band, it is a deliberate decision that needs its own ADR.
- Move to what is *needed*, not to the ceiling. The band is a permission, not
  a target: this change takes 1.95 (what wasmtime 48 requires), not 1.96
  (what the band would allow).

Rust ships every six weeks, so stable−2 is roughly a twelve-week grace period
for anyone building from source. Prebuilt artifacts — pip wheels, .deb/.rpm,
tarballs, Docker — are unaffected by MSRV at all.

Applying it now: stable is 1.98, so the band reaches 1.96. wasmtime 48.0.0
needs 1.95, comfortably inside. Taken.

## Consequences

- `rust-toolchain.toml` → `1.95.0`; workspace `rust-version` → `1.95`; twelve
  workflow pins, two deploy Dockerfiles and four install docs follow.
  `scripts/check-toolchain-consistency.sh` (added in #504) verified all sixteen
  in one pass — this is the first bump where nothing was missed by hand.
- **rustc 1.95 added `clippy::duration_suboptimal_units`**, which fires 90 times
  across the workspace: HotL expiry constants, dedup TTLs, retry/backoff
  windows, and a great many test fixtures that reason in seconds. Rewriting all
  of them inside a toolchain bump would bury a behaviour-relevant edit
  (a mistyped expiry constant) in mechanical churn, so the lint is suppressed
  and the rewrite left as its own decision.
  - Note the suppression had to go in **three** places: CI's `clippy`
    invocations get `-A`, and `xiaoguai-watch`, `xiaoguai-runtime` and
    `xiaoguai-scheduler` each need a crate-level `#![allow]` because they carry
    an in-source `#![warn(clippy::pedantic)]` that overrides both the workspace
    `[lints]` table and the command-line flag. The workspace `[lints]` table is
    **not** a reliable place to allow a lint for those crates.
- Fifteen genuine 1.95 lints were fixed rather than suppressed:
  `map(..).unwrap_or(false)` → `is_ok_and` (11 sites), `sort_by` → `sort_by_key`
  with `Reverse` (2), redundant `.into_iter()` in `zip`/`chain` (2), and three
  markdown-parser match arms collapsed into guards (safe: the `match` has a
  no-op `_ => {}` catch-all, so a failing guard falls through to the same
  behaviour).
- wasmtime 47 → 48 needed **no code changes**; the API surface we use is
  unchanged.
- The next bump should be routine. If it is not, this ADR is wrong and should
  be revisited.

## Alternatives considered

- **Track stable exactly.** Rejected: no grace period for source builders, and
  every six weeks becomes a forced migration.
- **Keep the reactive policy.** Rejected: it produced this exact situation
  three times, and the third time cost a closed PR pair that has to be reopened.
- **stable − 4** (today: 1.94, i.e. stay put). Rejected: that is where we
  already were, and it blocks in-band upgrades for no security benefit.

## References

- ADR-0021, ADR-0023 (superseded) — the two reactive bumps
- PRs #499, #500 — wasmtime 48 closed under the old rules
- PR #504 — `check-toolchain-consistency.sh`, which made this bump mechanical
