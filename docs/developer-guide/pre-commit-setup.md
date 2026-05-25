# Pre-commit setup

Xiaoguai uses [pre-commit](https://pre-commit.com/) to run linting, formatting
and manifest validation hooks locally before each commit.  The same checks run
in CI, so catching them early saves you a round-trip.

## Install

```bash
# 1. Install pre-commit (once per machine)
pip install pre-commit          # or: pipx install pre-commit

# 2. Register the hooks with your local git repo (once per clone)
cd <repo-root>
pre-commit install
```

After `pre-commit install` the hooks run automatically on every `git commit`.

## What runs

| Hook | Trigger | Speed |
|---|---|---|
| check-yaml | any `.yaml`/`.yml` (excl. helm templates + GH Actions) | fast |
| check-json | any `.json` | fast |
| check-added-large-files | any file > 1 MB | fast |
| end-of-file-fixer | any file | fast |
| trailing-whitespace | any file | fast |
| mixed-line-ending | any file | fast |
| rust-fmt | staged `.rs` files | fast (workspace-wide) |
| rust-clippy | staged `.rs` files | slow (per affected crate) |
| validate-pack | `packs/*/pack.yaml` | fast |
| validate-watcher | `packs/*/watches/*.yaml` | fast |
| validate-hotl-policy | `examples/hotl-policies/**` | fast |
| validate-recipe | `recipes/**` | fast |
| ts-typecheck | staged `.ts`/`.tsx` | medium (per package) |
| ts-eslint | staged `.ts`/`.tsx` | medium (per package) |
| gitleaks | all staged files | fast |

## Run manually

```bash
# Run all hooks against all files (useful after first install)
pre-commit run --all-files

# Run a single hook by id
pre-commit run rust-fmt --all-files
pre-commit run validate-pack --all-files

# Run hooks against a specific file
pre-commit run --files packs/incident-triage/pack.yaml
```

## Update hooks to latest revisions

```bash
pre-commit autoupdate
git add .pre-commit-config.yaml && git commit -m "chore: bump pre-commit hook revs"
```

## Troubleshooting

### `cargo fmt` or `cargo clippy` not found

Install Rust via [rustup](https://rustup.rs/) and ensure `~/.cargo/bin` is on
your `PATH`.  The repo pins the toolchain in `rust-toolchain.toml`; rustup will
download it automatically on first use.

### `pnpm` not found (ts-typecheck / ts-eslint)

Install pnpm:

```bash
npm install -g pnpm
# or
corepack enable pnpm
```

Then install frontend dependencies:

```bash
cd frontend && pnpm install
```

### `gitleaks` fails with "no such file or directory"

The gitleaks hook is managed by pre-commit and downloaded into its cache
automatically.  If the cache is corrupted run:

```bash
pre-commit clean
pre-commit run gitleaks --all-files
```

### Manifest validation fails with "missing required field"

The validator checks for `apiVersion`, `kind`, `metadata`, and `spec` at the
top level.  Example minimal valid `pack.yaml`:

```yaml
apiVersion: xiaoguai.ai/v1
kind: Pack
metadata:
  name: my-pack
spec: {}
```

### Hook is slow on first run

Pre-commit downloads and caches hook environments (Python virtualenvs, etc.)
on the first run.  Subsequent runs use the cache and are much faster.

## Emergency opt-out

If you need to commit urgently and the hooks are blocking you, bypass them
with `--no-verify`:

```bash
git commit --no-verify -m "wip: emergency fix"
```

**This bypasses ALL hooks including secret scanning.** Use only in genuine
emergencies and follow up by pushing to a draft PR so CI can run the checks.
Do not merge `--no-verify` commits to main without CI passing.
