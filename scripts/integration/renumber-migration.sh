#!/usr/bin/env bash
# renumber-migration.sh — safely rename a migration file and update references.
#
# Usage:
#   scripts/integration/renumber-migration.sh <old_name.sql> <new_name.sql>
#
# Example:
#   scripts/integration/renumber-migration.sh 0016_personas.sql 0018_personas.sql
#
# What it does:
#   1. Finds the migration file anywhere under the repo (recursive).
#   2. Renames it via `git mv`.
#   3. Updates references in .rs, .toml, .md, .txt, .sql files via sed.
#
# Pass --dry-run to preview without making changes.
set -euo pipefail

OLD_NAME="${1:-}"
NEW_NAME="${2:-}"
DRY_RUN=false
[[ "${3:-}" == "--dry-run" || "${1:-}" == "--dry-run" ]] && DRY_RUN=true

if [[ -z "$OLD_NAME" || -z "$NEW_NAME" ]]; then
    echo "Usage: renumber-migration.sh <old_name.sql> <new_name.sql> [--dry-run]"
    exit 1
fi

# Find the file
OLD_PATH=$(find . -name "$OLD_NAME" -not -path "./.git/*" | head -1)
if [[ -z "$OLD_PATH" ]]; then
    echo "ERROR: '$OLD_NAME' not found in repo."
    exit 1
fi

# Derive the new path by replacing the filename component
OLD_DIR=$(dirname "$OLD_PATH")
NEW_PATH="${OLD_DIR}/${NEW_NAME}"

echo "Rename: $OLD_PATH -> $NEW_PATH"

if [[ "$DRY_RUN" == "true" ]]; then
    echo "DRY-RUN: would run: git mv \"$OLD_PATH\" \"$NEW_PATH\""
else
    git mv "$OLD_PATH" "$NEW_PATH"
    echo "  git mv done."
fi

# Find and update references in source/doc files
EXTENSIONS=("rs" "toml" "md" "txt" "sql" "yaml" "yml")
EXT_PATTERN=$(IFS='|'; echo "${EXTENSIONS[*]}")

echo "Scanning for references to '$OLD_NAME'..."

while IFS= read -r -d '' file; do
    if grep -q "$OLD_NAME" "$file" 2>/dev/null; then
        echo "  Reference found: $file"
        if [[ "$DRY_RUN" == "true" ]]; then
            echo "    DRY-RUN: would replace '$OLD_NAME' -> '$NEW_NAME'"
        else
            # Use temp file to be safe on both macOS (BSD sed) and Linux (GNU sed)
            tmp=$(mktemp)
            sed "s/${OLD_NAME}/${NEW_NAME}/g" "$file" > "$tmp"
            mv "$tmp" "$file"
            echo "    Updated."
        fi
    fi
done < <(find . -not -path "./.git/*" -type f \( \
    -name "*.rs" -o -name "*.toml" -o -name "*.md" -o \
    -name "*.txt" -o -name "*.sql" -o -name "*.yaml" -o -name "*.yml" \
\) -print0)

if [[ "$DRY_RUN" == "true" ]]; then
    echo "Dry-run complete — no files changed."
else
    echo "Done. Stage with: git add -u"
fi
