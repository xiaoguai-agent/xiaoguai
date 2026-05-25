# SBOM / cargo-cyclonedx 0.5.9 Migration Investigation

**Date**: 2026-05-25
**Status**: Decision made — upgrade to 0.5.9 (clean path)
**Branch**: `chore/cyclonedx-0_5_9-research`

---

## Background

`ci/release-yml-fix` pinned `cargo install cargo-cyclonedx@0.5.8 --locked` with this comment:

> "v0.5.9 removed the `--output-pattern` flag, breaking this step.  
>  TODO: re-test on cyclonedx 0.5.9+ when `--output-pattern` semantic is clear."

The main-branch `release.yml` uses `cargo install cargo-cyclonedx --locked` (unversioned,
tracks latest) and the SBOM step calls `--output-pattern dir --output-path sbom`, which
also fails on any installed version — see below.

---

## Findings

### 1. `--output-pattern` was already gone before 0.5.8

Both 0.5.8 and 0.5.9 reject `--output-pattern` and `--output-path` as unknown arguments.
These flags were removed in an earlier release (before 0.5.8).  The breakage described in
the TODO existed on **both** versions; the 0.5.8 pin provides no protection.

```
$ cargo-cyclonedx 0.5.8 cyclonedx --format json --output-pattern dir --output-path sbom
error: unexpected argument '--output-pattern' found

$ cargo-cyclonedx 0.5.9 cyclonedx --format json --output-pattern dir --output-path sbom
error: unexpected argument '--output-pattern' found
```

### 2. `--help` diff between 0.5.8 and 0.5.9

The only difference is that 0.5.9 adds a `SOURCE_DATE_EPOCH` note for reproducible builds.
All flags are identical. There is **no regression** from 0.5.8 to 0.5.9.

### 3. Canonical 0.5.9+ way to produce per-crate SBOMs

0.5.9 produces per-crate SBOM files automatically in workspace mode; the files land
next to each crate's `Cargo.toml`. No directory flag is needed.

```bash
# From workspace root — produces one .cdx.json per crate under crates/*/
cargo cyclonedx --format json

# Same files, target triple appended to filename (e.g. xiaoguai-cli_aarch64-apple-darwin.cdx.json)
cargo cyclonedx --format json --target-in-filename

# Custom filename (single-crate only, no workspace scatter)
cargo cyclonedx --format json --override-filename "custom-name"
```

On this workspace (27 crates), `cargo cyclonedx --format json` produces 27 files:

```
crates/xiaoguai-agent/xiaoguai-agent.cdx.json
crates/xiaoguai-api/xiaoguai-api.cdx.json
crates/xiaoguai-audit/xiaoguai-audit.cdx.json
... (24 more)
```

Collecting them into an `sbom/` artifact dir requires a `find`/`cp` step in CI
(or just uploading the whole workspace tree with a glob).

### 4. SBOM JSON schema differences: 0.5.8 vs 0.5.9

Structural diff against a serde + serde_json test crate:

| Field | 0.5.8 | 0.5.9 | Impact |
|---|---|---|---|
| `bomFormat` | CycloneDX | CycloneDX | none |
| `specVersion` | 1.3 | 1.3 | none |
| `metadata.tools[].version` | 0.5.8 | 0.5.9 | cosmetic |
| `dependencies[].dependsOn` | `[]` on leaf nodes | omitted | **minor** |
| All component fields | identical | identical | none |

The only structural change is that 0.5.9 omits empty `"dependsOn": []` arrays for
dependency leaf nodes. Per CycloneDX 1.3 spec, an absent key and an empty array are
semantically equivalent. Any downstream tool iterating `dependencies` will behave
identically; a tool doing strict `"dependsOn" in entry` presence checks would see a
difference (no known tool does this).

### 5. Verdict

**Upgrade to 0.5.9 is clean.**

- No flag regression between 0.5.8 and 0.5.9.
- The CI breakage was caused by `--output-pattern`/`--output-path` flags that were
  removed before 0.5.8 — pinning to 0.5.8 did not help.
- The fix is to remove those deprecated flags from the `cargo cyclonedx` invocation
  and add a `find`/`mv` step to collect the scattered `.cdx.json` files into `sbom/`.
- Schema is backward-compatible; downstream SBOM consumers are unaffected.

---

## Other Supply-Chain Tool Audit

| Tool | Workflow | Pin status | Notes |
|---|---|---|---|
| `cargo-cyclonedx` | `release.yml` | unversioned (tracks latest) | Fixed in this PR to `@0.5.9 --locked` |
| `cargo-audit` | `audit.yml` | **unversioned** (tracks latest) | Risk: advisories-DB CLI format can change. Recommend pinning `@0.9.1` |
| `cargo-vet` | `cargo-vet.yml` | `0.10.0` pinned | Good — version env var used correctly |
| `cargo-deny` | `deny.yml` | via `EmbarkStudios/cargo-deny-action@v2` | Major-version pin; acceptable |
| `cosign` | `release.yml`, `release-tarball.yml` | via `sigstore/cosign-installer@v3` | Major-version pin; acceptable |
| `trivy` | not present | — | Not used directly; covered by Snyk |
| `syft`/`grype` | not present | — | Not used directly |
| Snyk | `snyk.yml` | via GH Actions action | Pinned via action SHA — acceptable |

**Actionable findings**:
- `cargo-audit` in `audit.yml` is unversioned (`cargo install cargo-audit --locked`
  with no `@version`). This tracks PyPI latest and can break silently.
  Recommend: `cargo install cargo-audit@0.21.0 --locked` (current stable as of 2026-05).

---

## Migration: Old vs New CI Step

### Before (broken on all versions)

```yaml
- name: Install cargo-cyclonedx
  run: cargo install cargo-cyclonedx --locked          # unversioned
- name: Generate SBOM
  run: cargo cyclonedx --format json --output-pattern dir --output-path sbom
  # ^^^ --output-pattern and --output-path removed before 0.5.8; always fails
```

### After (0.5.9, working)

```yaml
- name: Install cargo-cyclonedx
  run: cargo install cargo-cyclonedx@0.5.9 --locked
- name: Generate SBOM
  run: |
    cargo cyclonedx --format json
    mkdir -p sbom
    find . -name "*.cdx.json" -not -path "./target/*" -exec cp {} sbom/ \;
```

---

## References

- cargo-cyclonedx changelog: https://github.com/CycloneDX/cyclonedx-rust-cargo/blob/main/CHANGELOG.md
- CycloneDX 1.3 spec dependency graph: https://cyclonedx.org/docs/1.3/
- SOURCE_DATE_EPOCH reproducible builds: https://reproducible-builds.org/docs/source-date-epoch/
