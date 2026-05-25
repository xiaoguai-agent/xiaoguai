# Mutation Testing — Wave-3 Capability Evals

## What mutation testing reveals

Mutation testing answers a question that code coverage cannot: **do your assertions actually catch logic regressions?**

`cargo-mutants` makes small, syntactically valid changes to production code (mutants) — flipping `&&` to `||`, negating a boolean, changing a comparison operator, replacing a return value — then re-runs the test suite. A mutant that **survives** (all tests still pass under mutated code) exposes a gap: the eval either does not exercise that code path, or it exercises it but does not assert the outcome strongly enough.

For wave-3 this matters because the eval suites (`dsl_eval.rs`, `accuracy_eval.rs`, `hotl_eval.rs`, `outcomes_eval.rs`) are capability evals, not unit tests. They verify higher-level properties. Mutations in low-level logic (e.g., `policy.rs::is_breach`, `baseline.rs::ewma_update`) can pass undetected if the eval inputs do not hit the boundary case the mutation changes.

## Output categories

| Category | Meaning | Action |
|---|---|---|
| **caught** | Test suite failed under the mutant — the eval detected the regression | None required |
| **missed** | Test suite passed under the mutant — eval does not catch this logic change | Strengthen assertion or add boundary fixture |
| **unviable** | Mutant did not compile | Ignore — not a real regression surface |

The **kill rate** is `caught / (caught + missed)`. The threshold for this project is **80%**.

## How to interpret a surviving mutant

Example output from a missed mutant in `policy.rs`:

```
MISSED  crates/xiaoguai-api/src/hotl/policy.rs:47:12
  replace `>=` with `>`
  in fn is_breach(score: f64, threshold: f64) -> bool
```

This means `hotl_eval.rs` never tests the exact-boundary case (`score == threshold`). Fix: add a fixture where the input score equals the configured threshold and assert it is treated as a breach.

A surviving mutant in `detector.rs` that replaces `ewma_alpha` with `0.0` means the accuracy eval only tests the directional trend, not the magnitude — add a fixture that verifies a specific numeric output.

## When to run

| Trigger | Recommended? | Notes |
|---|---|---|
| Per-PR | **No** — too slow | Mutation runs take 20-60 min per crate; use unit + integration tests for PR gating |
| **Nightly cron** | **Yes** | GitHub Actions workflow runs at 08:00 UTC on main |
| **Pre-release** | **Yes** | Run before cutting a release tag to verify eval health |
| After adding a new eval | Yes | Verify the new eval actually kills mutations in the module it covers |

## Running locally

```bash
# Install (one-time)
cargo install cargo-mutants@25.0.1

# Run for wave-3 targeted crates only
bash tests/mutation/wave3/run-mutation.sh

# Inspect HTML report
open mutation-report/xiaoguai-watch/index.html
open mutation-report/xiaoguai-anomaly/index.html
open mutation-report/xiaoguai-api/index.html
open mutation-report/xiaoguai-audit/index.html

# Machine-readable kill-rate summary
cat mutation-report/kill-rate.json
```

The `--in-diff origin/main` flag (used by default) restricts mutations to lines changed since `origin/main`. For a full baseline run (e.g., first-time setup), edit `run-mutation.sh` and remove that flag.

## Targeted files and their eval cross-references

| Source file (mutated) | Eval file (must catch mutations) |
|---|---|
| `crates/xiaoguai-watch/src/spec.rs` | `crates/xiaoguai-watch/tests/dsl_eval.rs` |
| `crates/xiaoguai-watch/src/dedup.rs` | `crates/xiaoguai-watch/tests/dsl_eval.rs` |
| `crates/xiaoguai-anomaly/src/detector.rs` | `crates/xiaoguai-anomaly/tests/accuracy_eval.rs` |
| `crates/xiaoguai-anomaly/src/baseline.rs` | `crates/xiaoguai-anomaly/tests/accuracy_eval.rs` |
| `crates/xiaoguai-api/src/hotl/enforcer.rs` | `crates/xiaoguai-api/tests/hotl_eval.rs` |
| `crates/xiaoguai-api/src/hotl/policy.rs` | `crates/xiaoguai-api/tests/hotl_eval.rs` |
| `crates/xiaoguai-audit/src/outcomes.rs` | `crates/xiaoguai-audit/tests/outcomes_eval.rs` |

## Common patterns where mutations survive

**`policy.rs::is_breach` boundary**
Mutations that shift `>=` to `>` survive when no eval fixture uses `score == threshold`. Add an exact-boundary test case.

**`baseline.rs` EWMA alpha**
Mutations replacing the alpha constant survive when evals only check directional trends. Add a fixture asserting a specific numeric output after N samples.

**`dedup.rs` window comparison**
Off-by-one mutations (`>` vs `>=` on time window edge) survive when fixtures use gaps far from the boundary. Add a fixture at `window_size - 1` and `window_size` intervals.

**`outcomes.rs` attribution chain**
Mutations that short-circuit multi-step attribution survive when evals only check the final outcome label, not intermediate attributions. Assert the full attribution chain.

## Why 80%?

80% was chosen as a pragmatic threshold for capability evals:

- **Not 100%**: Some surviving mutants are in defensive/unreachable branches that exist for safety, not correctness. Forcing 100% would require eval fixtures for adversarial inputs outside the normal operating envelope.
- **Not lower**: Below 80% means more than 1 in 5 tested logic mutations goes undetected, which is too weak for the policy enforcement and anomaly detection modules where correctness is safety-critical.
- **Revisit after first baseline run**: Once `expected-baseline.json` is populated with real numbers, per-crate thresholds can be tuned. `xiaoguai-api/hotl` may warrant a higher bar (90%+) given its policy-enforcement role.
