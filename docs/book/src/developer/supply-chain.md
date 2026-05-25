# Supply Chain Security

{{#include ../../../runbooks/cargo-vet.md:1:}}

---

## cargo-deny

Licence and duplicate-crate gating lives in `deny.toml`. The `deny.yml`
CI workflow runs on every PR. Configuration follows `deny.toml` in the
repository root.

{{#include ../../../runbooks/dependabot.md:1:}}
