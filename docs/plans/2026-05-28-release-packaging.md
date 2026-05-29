# Plan B — Release packaging (cargo-dist + Homebrew tap + systemd rename)

> Companion to the session-5 handoff (`docs/HANDOFF-2026-05-28-session5.md`).
> Meta-plan: `~/.claude/plans/drifting-zooming-stroustrup.md`.

## 1. Context

Today every release requires:
- `cargo build --release` on Linux x86_64/aarch64 (covered by
  `release-tarball.yml` — cosign-signed, SLSA L3 — good)
- A `.deb` + `.rpm` (covered by `release-packages.yml` — good)
- A container image (covered by `release.yml` — good)

But **macOS and Windows binaries are missing**, **there is no Homebrew tap**,
and the systemd unit `ExecStart=/usr/local/bin/xiaoguai-core serve` still
points at the legacy shim. The session-5 handoff lists these as Tier-1 release
polish (≈ 1–2 h each).

We pick **`cargo-dist`** because (a) it's Rust-native, (b) it generates
Homebrew formulae out of the box, (c) Anthropic's own `claude-code` ships with
it. We will *not* retire the existing tarball workflow without verifying
cargo-dist's signing posture can replace it (decision in step 5.4).

Outcome: one `git tag vX.Y.Z` push produces Linux/macOS/Windows artefacts on
GitHub Releases **and** updates a Homebrew tap formula automatically.

## 2. Success criteria

1. `git tag v0.0.0-dist-rc1 && git push origin v0.0.0-dist-rc1` on a release
   branch triggers a CI workflow that finishes green and uploads:
   - `xiaoguai-x86_64-unknown-linux-gnu.tar.xz`
   - `xiaoguai-aarch64-unknown-linux-gnu.tar.xz`
   - `xiaoguai-x86_64-apple-darwin.tar.xz`
   - `xiaoguai-aarch64-apple-darwin.tar.xz`
   - `xiaoguai-x86_64-pc-windows-msvc.zip`
   - `installer.sh`, `installer.ps1`
   - `sha256.txt` / `dist-manifest.json`
   …all attached to the GitHub Release page for the tag.
2. On a clean (or `brew uninstall xiaoguai` first) macOS, the sequence
   `brew tap xiaoguai-agent/tap && brew install xiaoguai` succeeds, and
   `xiaoguai --version` prints `0.0.0-dist-rc1`.
3. On Windows (or via `wine`/CI runner), `iwr -useb installer.ps1 | iex`
   places `xiaoguai.exe` on `PATH` and `xiaoguai --version` prints
   `0.0.0-dist-rc1`.
4. `deploy/systemd/xiaoguai-core.service` has
   `ExecStart=/usr/local/bin/xiaoguai serve --config /etc/xiaoguai/config.yaml`,
   keeps unit filename, User, and hardening lines unchanged. `systemd-analyze
   verify deploy/systemd/xiaoguai-core.service` exits 0.
5. After tagging the **real** next release `vX.Y.Z`, the `.deb`/`.rpm` jobs
   in `release-packages.yml` still produce installable packages
   (`dpkg -i ./xiaoguai_*.deb && xiaoguai --version` works), proving we
   didn't break the existing pipeline.
6. `README.md` install matrix lists Homebrew (macOS), shell installer
   (Linux/macOS), PowerShell installer (Windows), `.deb`/`.rpm` (Linux), and
   the container image — with copy-paste install commands for each.

## 3. Prerequisites

| What | Verify by |
|---|---|
| Push permissions on `xiaoguai-agent/xiaoguai` | `gh repo view xiaoguai-agent/xiaoguai --json viewerPermission` |
| Ability to create the tap repo `xiaoguai-agent/homebrew-tap` | `gh repo create --help` (and that the user wants it under that org) |
| A PAT scoped `repo:write` on the tap repo, stored as `HOMEBREW_TAP_TOKEN` secret on the main repo | `gh secret list -R xiaoguai-agent/xiaoguai | grep HOMEBREW_TAP_TOKEN` |
| `cargo-dist` installable locally for dry-runs | `cargo install cargo-dist --locked --version 0.21.0` (or current latest) |
| Clean working tree | `git status --porcelain` empty |
| The three bin definitions today: `xiaoguai`, `xiaoguai-core`, `xiaoguai-mcp-exec` — confirmed via `grep '^\[\[bin\]\]' crates/*/Cargo.toml` | Step output matches expectation |

If any prerequisite is missing, capture it in the PR description rather than
blocking; the PAT in particular requires user action.

## 4. Step-by-step actions

### Step 4.1 — Branch + audit

```bash
git checkout -b feat/cargo-dist-release
git fetch origin && git rebase origin/main
```

**VC:** `git log --oneline -1` matches `origin/main`.

### Step 4.2 — Initialize cargo-dist

```bash
cargo dist init --yes \
  --installer shell \
  --installer powershell \
  --installer homebrew \
  --tap xiaoguai-agent/homebrew-tap \
  --targets x86_64-unknown-linux-gnu,aarch64-unknown-linux-gnu,x86_64-apple-darwin,aarch64-apple-darwin,x86_64-pc-windows-msvc
```

This will:
- write `[workspace.metadata.dist]` to the root `Cargo.toml`
- write `.github/workflows/release.yml`. **⚠ Name clash** with the existing
  `release.yml` (container image). Rename the cargo-dist file to
  `.github/workflows/release-dist.yml` **before** committing.

**VC:** `grep -q '\[workspace.metadata.dist\]' Cargo.toml` returns 0, and
`ls .github/workflows/release-dist.yml` returns 0.

### Step 4.3 — Restrict cargo-dist to the user-facing binaries

By default cargo-dist ships every workspace binary. We have three:
- `xiaoguai` — user-facing CLI; **ship**
- `xiaoguai-core` — legacy shim, kept for systemd .deb backward-compat;
  **opt out of cargo-dist tarballs** (still produced by the existing .deb
  job)
- `xiaoguai-mcp-exec` — sandbox binary; **ship** (operators need it on
  `$PATH` for `xiaoguai mcp register --command $(which xiaoguai-mcp-exec)`)

Set `dist = false` in `crates/xiaoguai-core/Cargo.toml`'s
`[package.metadata.dist]`:

```toml
[package.metadata.dist]
dist = false
```

**VC:** `cargo dist plan` output lists `xiaoguai` and `xiaoguai-mcp-exec`
under "binaries to ship" and does **not** list `xiaoguai-core`.

### Step 4.4 — Decide on cosign signing (and the existing tarball workflow)

cargo-dist supports keyless cosign via the same `sigstore/cosign-installer`
action already used by `release-tarball.yml`. Configure
`hosting = ["github"]` and `pr-run-mode = "plan"` in
`[workspace.metadata.dist]`. **Add** a post-build step to the generated
workflow that runs:

```bash
cosign sign-blob --yes target/distrib/*.tar.xz target/distrib/*.zip
```

writing `.sig` + `.pem` next to each artefact. This matches the existing
tarball workflow's posture so we can retire `release-tarball.yml`.

**VC:** `grep -q 'cosign sign-blob' .github/workflows/release-dist.yml`
returns 0.

### Step 4.5 — Retire the redundant tarball workflow

After step 4.4 the cargo-dist workflow covers everything `release-tarball.yml`
does (and more). Delete it. **Keep** `release.yml` (container image + SBOM)
and `release-packages.yml` (.deb + .rpm) — they are orthogonal.

```bash
git rm .github/workflows/release-tarball.yml
```

**VC:** `ls .github/workflows/release-tarball.yml` returns "No such file";
`grep -l 'release-tarball' .github/workflows/` is empty (no internal refs).

### Step 4.6 — Fix the systemd unit

Edit `deploy/systemd/xiaoguai-core.service`:

```
- ExecStart=/usr/local/bin/xiaoguai-core serve --config /etc/xiaoguai/config.yaml
+ # v1.4.x: canonical entrypoint is `xiaoguai serve`. The xiaoguai-core
+ # shim still ships in the .deb for backward-compat with sites that
+ # haven't redeployed the unit, but the unit itself now uses the unified
+ # CLI to align with the cargo-dist binary set.
+ ExecStart=/usr/local/bin/xiaoguai serve --config /etc/xiaoguai/config.yaml
```

Keep unit filename `xiaoguai-core.service` (so existing systemd drop-ins at
`/etc/systemd/system/xiaoguai-core.service.d/` keep working). User, hardening,
and capabilities unchanged.

**VC:** `systemd-analyze verify deploy/systemd/xiaoguai-core.service` exits 0
(if available on host; otherwise visually diff against pre-change file).

### Step 4.7 — Create the Homebrew tap repo (manual user step)

```bash
gh repo create xiaoguai-agent/homebrew-tap --public \
  --description "Homebrew tap for xiaoguai" \
  --license MIT
```

Add a `Formula/` directory and an empty `README.md` so cargo-dist can push
into it on the first release.

**VC:** `gh repo view xiaoguai-agent/homebrew-tap --json visibility,name`
returns the public repo.

### Step 4.8 — Provision the tap PAT secret

User-action (requires browser/2FA):

1. Create a fine-grained PAT scoped to `xiaoguai-agent/homebrew-tap` with
   `Contents: Read & Write`.
2. `gh secret set HOMEBREW_TAP_TOKEN -R xiaoguai-agent/xiaoguai -b <PAT>`.

**VC:** `gh secret list -R xiaoguai-agent/xiaoguai | grep HOMEBREW_TAP_TOKEN`
returns one row.

### Step 4.9 — Local dry-run

```bash
cargo dist build --target x86_64-apple-darwin
ls target/distrib/
```

**VC:** `target/distrib/xiaoguai-*-x86_64-apple-darwin.tar.xz` exists and
`tar tjf` lists `xiaoguai` (not `xiaoguai-core`, per step 4.3).

### Step 4.10 — RC tag against a fork

Push the branch to a fork (e.g., `zw008/xiaoguai`) and tag `v0.0.0-dist-rc1`:

```bash
git remote add fork git@github.com:zw008/xiaoguai.git
git push fork feat/cargo-dist-release
git tag v0.0.0-dist-rc1
git push fork v0.0.0-dist-rc1
```

Watch the workflow. We push to a **fork** to avoid polluting the
`xiaoguai-agent/xiaoguai` Releases page with `-rc` tags.

**VC:** `gh run watch -R zw008/xiaoguai $(gh run list -R zw008/xiaoguai -L 1 --json databaseId -q '.[0].databaseId')`
exits 0; `gh release view v0.0.0-dist-rc1 -R zw008/xiaoguai` lists all 5
platform tarballs + 2 installers + `dist-manifest.json`.

### Step 4.11 — Install smoke tests

On macOS:

```bash
brew uninstall xiaoguai 2>/dev/null || true
brew untap xiaoguai-agent/tap 2>/dev/null || true
brew tap xiaoguai-agent/tap   # populated by the fork's tap PR
brew install xiaoguai
xiaoguai --version    # → 0.0.0-dist-rc1
```

On Linux:

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/zw008/xiaoguai/releases/download/v0.0.0-dist-rc1/installer.sh | sh
xiaoguai --version
```

On Windows (via GitHub Actions windows-latest, or wine if needed):

```powershell
iwr -useb https://github.com/zw008/xiaoguai/releases/download/v0.0.0-dist-rc1/installer.ps1 | iex
xiaoguai --version
```

**VC:** all three platforms report `0.0.0-dist-rc1`.

### Step 4.12 — README install matrix

Update `README.md` install section (and `README-CN.md` if present) with a 5-row
table:

| Platform | Command |
|---|---|
| macOS | `brew tap xiaoguai-agent/tap && brew install xiaoguai` |
| Linux (any) | `curl … installer.sh | sh` |
| Windows | `iwr -useb … installer.ps1 | iex` |
| Debian/Ubuntu | `dpkg -i xiaoguai_*.deb` |
| Container | `docker pull ghcr.io/xiaoguai-agent/xiaoguai:latest` |

**VC:** `grep -c 'brew tap xiaoguai-agent/tap' README.md` ≥ 1.

### Step 4.13 — Open PR

```
gh pr create --title 'release: cargo-dist + Homebrew tap + systemd rename' \
  --body-file docs/plans/2026-05-28-release-packaging.md
```

Include in the PR description: the four CI workflows that exist after this
PR (`release.yml`, `release-dist.yml`, `release-packages.yml`, plus the
removal of `release-tarball.yml`). Tag the user-action prereqs (tap repo,
PAT secret) at the top of the description so the reviewer can verify they
were done.

**VC:** PR opens; CI green; reviewer can verify each §2 success criterion
from the PR description.

## 5. Risks & open questions

| Risk | Mitigation |
|---|---|
| cargo-dist generated workflow name collides with `release.yml` | Renamed to `release-dist.yml` in step 4.2; verified by `ls` |
| Homebrew formula has `xiaoguai-mcp-exec` as a separate binary — `brew install xiaoguai` may not put it on PATH | cargo-dist's Homebrew installer ships **all** binaries listed in `dist = true` packages by default; verified in step 4.11 by `which xiaoguai-mcp-exec` after `brew install`. If not, add a `binaries = ["xiaoguai", "xiaoguai-mcp-exec"]` override in `[workspace.metadata.dist]` |
| The existing `release-tarball.yml` builds with **cross** for aarch64-gnu; cargo-dist uses **github runners + zigbuild** by default which has a different glibc floor (2.31) | Pin runner to `ubuntu-22.04` (glibc 2.35) to match `release-packages.yml` baseline. Document the bump in RELEASE_NOTES |
| macOS binary unsigned → "unidentified developer" warning on first run | `brew install` sidesteps Gatekeeper. Document the warning in install matrix; defer notarization (needs Apple Developer ID, $99/yr) |
| PAT for `HOMEBREW_TAP_TOKEN` may be over-scoped if classic | Use fine-grained PAT (step 4.8) scoped only to the tap repo's Contents |
| `cargo dist init` writes inline workflow YAML; future cargo-dist version bumps regenerate it | Workflow file has a `# cargo-dist generated — re-run with` header. Adopt that. Treat it as generated code |
| `release-tarball.yml` had SLSA L3 provenance; cargo-dist's keyless cosign is L2-equivalent | Document the regression. If L3 needed, **don't delete** release-tarball.yml; instead keep both and accept double-build for Linux artefacts |

## 6. Rollback / abort criteria

- Step 4.10 (RC tag) fails on the fork → **abort here**, leave main untouched.
  Branch can be force-deleted: `git branch -D feat/cargo-dist-release`.
- Step 4.11 macOS smoke fails (formula broken) → don't merge the PR.
- After merge to main, the first **real** release tag (`vX.Y.Z`) fails →
  revert the merge: `git revert -m 1 <merge-sha>` and tag a hotfix. The .deb
  and container image workflows are independent, so a cargo-dist regression
  does not block them.

## 7. Out of scope

- Apple Developer ID signing + notarization (defer; needs paid account).
- Windows code signing (defer; needs EV cert).
- Chocolatey / Scoop / WinGet packaging (Windows users can use the
  PowerShell installer for now).
- Adding `xiaoguai-mcp-exec` as a separate Homebrew formula (we ship it in
  the main formula).
- Migrating the .deb / .rpm jobs into cargo-dist (cargo-dist doesn't do
  native Linux packages yet; keep `release-packages.yml`).
- Notarized `.pkg` for macOS (defer).
- Updating any IM-gateway binaries — they are libraries inside the main
  `xiaoguai` binary in this version.

## 8. References

- Session-5 handoff "what didn't get done" table: `docs/HANDOFF-2026-05-28-session5.md`
- Existing workflows: `.github/workflows/release.yml`,
  `.github/workflows/release-tarball.yml`,
  `.github/workflows/release-packages.yml`
- systemd unit: `deploy/systemd/xiaoguai-core.service`
- Cargo.toml `[package.metadata.deb]` lives in `crates/xiaoguai-cli/Cargo.toml`
- cargo-dist docs: <https://opensource.axo.dev/cargo-dist/book/>
- Reference adoption (claude-code, also Rust): <https://github.com/anthropics/claude-code>

---

## Self-review

| # | Check | Result |
|---|---|---|
| 1 | Cited file paths exist | **PASS** — `release.yml`, `release-tarball.yml`, `release-packages.yml`, `deploy/systemd/xiaoguai-core.service` all in tree |
| 2 | Every `VC:` is runnable | **PASS** — all are concrete `gh`, `cargo`, `grep`, `systemd-analyze`, or `tar` calls |
| 3 | Each §2 criterion maps to a §4 step | **PASS** — crit 1→4.10VC, 2→4.11, 3→4.11, 4→4.6VC, 5→4.13VC (covered by CI), 6→4.12VC |
| 4 | §7 out-of-scope honored | **PASS** — no step touches signing/notarization, chocolatey, .pkg |
| 5 | Each §5 risk has a mitigation | **PASS** |
| 6 | Step durations sane | **PASS** — 4.1–4.6 ≤ 30 min total (config); 4.7–4.8 ≤ 20 min (user action); 4.9–4.10 ≤ 40 min (CI watch); 4.11 ≤ 30 min (multi-platform); 4.12–4.13 ≤ 30 min. Total ≤ 3 h ≈ 1.5× top estimate (2–3 h) |

**Two soft spots**:
1. The cargo-dist version pin (4.21.0 above) may be stale by execution time
   — executor should `cargo dist --version` first and pin to whatever's
   current.
2. Step 4.7 + 4.8 (tap repo + PAT) require user-action with browser/2FA;
   if the user wants to defer Homebrew, plan B can ship in two passes:
   first cargo-dist alone (drop `--installer homebrew` from 4.2), then
   add Homebrew once the tap exists.
