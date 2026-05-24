# Runbook: cargo-vet supply chain attestation

**Added:** v1.1.8.1  
**Status:** report-only gate (see [Upgrading to blocking](#upgrading-to-blocking))

---

## What is cargo-vet?

[cargo-vet](https://mozilla.github.io/cargo-vet/) (Mozilla) verifies that every
dependency in `Cargo.lock` is either:

1. **Audited by a trusted import** — mozilla, bytecode-alliance, google, or embark-studios
2. **Audited by a maintainer** — entry in `supply-chain/audits.toml` `[audits]`
3. **Explicitly exempted** — entry in `supply-chain/audits.toml` `[exemptions]`

At bootstrap (v1.1.8.1) all 493 external (name, version) pairs are in category 3.
The goal is to migrate them to categories 1 or 2 over time.

---

## File layout

```
supply-chain/
├── config.toml      — policy + trusted import sources
├── audits.toml      — [trusted], [audits] (first-party), [exemptions] (bootstrap)
└── imports.lock     — locked snapshots of imported audits (auto-managed, commit this)
```

**Never edit `imports.lock` by hand.** It is managed by `cargo vet fetch-imports`.

---

## Day-to-day workflows

### 1. Adding a new dependency

When you add a crate to `Cargo.toml` and run `cargo update`:

```bash
# See what's new and unaudited:
cargo vet suggest

# Option A — audit it yourself (preferred):
cargo vet inspect <crate> <version>  # opens source in browser
# Then add to supply-chain/audits.toml manually (see format below)

# Option B — add an exemption (fast path for low-risk/build-only crates):
cargo vet add-exemption <crate> <version>
# This auto-appends to supply-chain/audits.toml [exemptions]

# Option C — the imported sources already cover it:
# Run `cargo vet` — if no error, you're done.
```

Always commit `supply-chain/audits.toml` alongside `Cargo.lock`.

### 2. Refreshing imported audits

The `imports.lock` file pins the content of the four imported audit sources at a
known-good commit.  To update to the latest:

```bash
cargo vet fetch-imports
git add supply-chain/imports.lock
git commit -m "chore: refresh cargo-vet imports.lock"
```

This is done automatically by the CI workflow on every run (with `continue-on-error`
so a temporarily unavailable remote doesn't break CI).

### 3. Running vet locally

```bash
# Install (one-time):
cargo install cargo-vet --version 0.10.0 --locked

# Check current state:
cargo vet --locked

# See what's unaudited and which imported sources might cover it:
cargo vet suggest

# Generate a diff of audits needed vs available:
cargo vet diff <crate> <old-version> <new-version>
```

### 4. Writing an audit entry

Add to `supply-chain/audits.toml` under `[audits]`:

```toml
[[audits.<crate-name>]]
who = "Your Name <your@email.example>"
criteria = "safe-to-deploy"
version = "1.2.3"
notes = """
Reviewed on 2026-XX-XX.
- No unsafe blocks outside of clearly bounded FFI wrappers.
- No network I/O or filesystem writes outside expected API surface.
- No obviously malicious code.
- Source matches published crates.io tarball (cargo vet inspect confirmed).
"""
```

Criteria to choose from:
- `safe-to-deploy` — safe to ship in a production binary
- `safe-to-run` — safe to run locally (weaker; for dev/test-only deps)
- `does-not-implement-crypto` — useful for crates that could be confused with crypto

### 5. Removing an exemption after auditing

Once you've added a first-party audit (or the crate is covered by an import), delete
the corresponding `[[exemptions.<crate>]]` block from `supply-chain/audits.toml`.

```bash
# Verify the exemption is no longer needed:
cargo vet --locked
# If it passes, the import or your new audit covers it — remove the exemption block.
```

---

## Upgrading to blocking

The CI gate is currently `continue-on-error: true`.  To make it block PR merges:

1. **Check coverage:** run `cargo vet suggest` locally.  This lists crates that have no
   audit from any source (first-party, imported, or exempted).

2. **Resolve all suggestions:** either add first-party audits or exemptions.

3. **Flip the flag** in `.github/workflows/cargo-vet.yml`:
   ```yaml
   # Change:
   continue-on-error: true   # REPORT-ONLY
   # To:
   continue-on-error: false  # BLOCKING
   ```

4. Open a PR; verify the workflow passes on `main`.

5. Add branch protection rule requiring the `cargo vet` status check to pass.

---

## Quarterly review checklist

- [ ] Run `cargo vet suggest` — are there unexempted crates?
- [ ] Run `cargo vet fetch-imports` — is `imports.lock` up to date?
- [ ] Review `[exemptions]` in `audits.toml` — can any be converted to audits or
      dropped because an import now covers them?
- [ ] Check for crates that have changed ownership on crates.io (supply-chain risk).
- [ ] Review the cargo-audit ignore list in `deny.toml` — drop any entry where an
      upstream fix is now available.

---

## Relationship to other security tooling

| Tool | What it checks | When it runs |
|------|---------------|--------------|
| **cargo-audit** (`audit.yml`) | Known CVE/advisory database | Nightly cron |
| **cargo-deny** (`deny.yml`) | License + bans + advisories | Every PR |
| **cargo-vet** (`cargo-vet.yml`) | Supply chain attestation | Every PR (report-only) |
| **Snyk** (`snyk.yml`) | Third-party vuln DB | Every PR |

cargo-vet is complementary to cargo-audit/deny: it addresses the question
"has a human looked at this crate?" rather than "is there a known advisory?".

---

## Troubleshooting

**`cargo vet` fails with "crate X version Y not found in audits or exemptions"**  
→ A new transitive dependency was added.  Run `cargo vet suggest` then either
audit it or add an exemption.

**`cargo vet fetch-imports` fails with a network error**  
→ The imported remote is temporarily unavailable.  CI falls back to the committed
`imports.lock`.  Retry locally later; do not remove the import.

**`imports.lock` shows unexpected diffs after `fetch-imports`**  
→ An upstream audit source was updated.  Review the diff (`git diff supply-chain/imports.lock`)
to confirm the changes look like normal audit additions, then commit.

**`cargo vet` passes locally but fails in CI**  
→ Likely a version skew between local cargo-vet and the pinned `CARGO_VET_VERSION`
in the workflow.  Match versions with `cargo install cargo-vet --version 0.10.0 --locked`.
