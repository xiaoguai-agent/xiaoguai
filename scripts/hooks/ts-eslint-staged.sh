#!/usr/bin/env bash
# ESLint hook — runs eslint --fix on staged .ts/.tsx files, scoped to
# the owning frontend package.
#
# pre-commit passes staged filenames as arguments (relative to repo root).
# ESLint is invoked from the package directory so it picks up the local
# eslint.config.*  / .eslintrc.* and tsconfig paths correctly.
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"

# Group staged files by their frontend package directory.
declare -A pkg_files  # pkg_dir -> space-separated relative paths from pkg_dir

for staged_file in "$@"; do
  # Only process files under frontend/
  if [[ "$staged_file" =~ ^(frontend/[^/]+)/ ]]; then
    pkg_dir="${BASH_REMATCH[1]}"
    rel_path="${staged_file#"$pkg_dir"/}"
    pkg_files["$pkg_dir"]+=" $rel_path"
  fi
done

if [[ ${#pkg_files[@]} -eq 0 ]]; then
  echo "[ts-eslint] No frontend .ts/.tsx files staged — skipping."
  exit 0
fi

failed=0
for pkg_dir in "${!pkg_files[@]}"; do
  abs_pkg="$REPO_ROOT/$pkg_dir"
  # shellcheck disable=SC2206
  files=(${pkg_files[$pkg_dir]})
  echo "[ts-eslint] Running eslint --fix in $pkg_dir on ${#files[@]} file(s)"
  cd "$abs_pkg"
  # Prefer local npx so the hook works without a global eslint install.
  npx eslint --fix "${files[@]}" || failed=1
  cd "$REPO_ROOT"
done

exit $failed
