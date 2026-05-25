# Integration Merge Resolution Toolkit

Scripts for resolving merge conflicts during parallel-branch integration convoys.

## Scripts

| Script | Purpose |
|---|---|
| `keep-both.py` | Auto-resolve text conflicts by keeping both HEAD and incoming sides |
| `merge-catalog.py` | Merge `catalog/skill_packs.json` from multiple branches, dedup by `slug` |
| `renumber-migration.sh` | Safely rename a migration SQL file and update all references |
| `merge-summary.py` | Merge conflicting `docs/book/src/SUMMARY.md`, dedup by (title, link) |
| `dedupe-package-json.py` | Fix duplicate keys in `frontend/admin-ui/package.json` |
| `merge-checkpoint.sh` | Single-branch merge with auto-resolution + `cargo check` gate |
| `integration-driver.sh` | Ordered convoy driver — iterates merge-checkpoint.sh, halts on failure |

## Quick Start

```bash
# Full convoy (branches in dependency order):
bash scripts/integration/integration-driver.sh \
    feat/hotl-policy-store \
    feat/outcome-recorder \
    feat/skill-pack-repository

# Single branch dry-run:
bash scripts/integration/merge-checkpoint.sh feat/my-branch --dry-run

# Resolve conflicts in current working tree:
python scripts/integration/keep-both.py
```

## Expected Failure Modes

### Unresolvable conflict (keep-both.py returns 1)
The conflict markers don't match the standard pattern (e.g., nested markers, binary files).
```bash
git merge --abort
# Fix manually, then:
git add -u && git commit -m "merge: <branch> (manual)"
```

### cargo check fails after merge
A Rust compilation error was introduced by the merge.
```bash
git merge --abort
# Fix the Rust error in the feature branch first:
git checkout feat/my-branch
# ... fix ...
git checkout -
bash scripts/integration/merge-checkpoint.sh feat/my-branch
```

### Cargo.lock regen fails
Dependency conflict between branches.
```bash
git merge --abort
# Update Cargo.toml in the feature branch to resolve version conflicts.
```

### pnpm-lock.yaml conflict
merge-checkpoint.sh will flag this but not auto-resolve.
```bash
pnpm install   # regenerates pnpm-lock.yaml
git add pnpm-lock.yaml
```

## Recovery After Convoy Halt

integration-driver.sh prints recovery commands on failure. General pattern:

```bash
# 1. Abort the failed merge
git merge --abort

# 2. Fix the issue (see failure mode above)

# 3. Rerun with remaining branches only
bash scripts/integration/integration-driver.sh feat/remaining-branch-a feat/remaining-branch-b
```

## Lock File Handling

- **Cargo.lock**: deleted and regenerated via `cargo generate-lockfile` in merge-checkpoint.sh.
- **pnpm-lock.yaml**: flagged for manual `pnpm install` — not auto-resolved.
- Both are skipped by `keep-both.py` (no regex substitution attempted).
