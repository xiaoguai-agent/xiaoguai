#!/usr/bin/env bash
# Every required status check must live in a workflow that also fires on
# `merge_group`, or the merge queue hangs forever.
#
# Why: the queue builds each entry on a temporary `gh-readonly-queue/...`
# branch and fires `merge_group`, NOT `pull_request`. A required check whose
# workflow does not listen for it can never report, so the entry waits until
# the 90-minute timeout and every subsequent merge is stuck behind it. This
# was nearly shipped when the queue was first enabled (#501 added the triggers
# just in time); the invariant is easy to break later by adding a fourth
# required check and forgetting the trigger.
#
# Reads the required checks from the branch ruleset so the script cannot drift
# from the actual configuration.
set -uo pipefail

REPO="${GITHUB_REPOSITORY:-xiaoguai-agent/xiaoguai}"
WF_DIR=".github/workflows"
fail=0

contexts=$(gh api "repos/${REPO}/rulesets" --jq '.[].id' 2>/dev/null | while read -r id; do
  gh api "repos/${REPO}/rulesets/${id}" \
    --jq '.rules[]? | select(.type=="required_status_checks") | .parameters.required_status_checks[].context' 2>/dev/null
done | sort -u)

if [ -z "$contexts" ]; then
  echo "No required status checks configured — nothing to guard."
  exit 0
fi

echo "Required status checks and the workflow that must carry merge_group:"
echo

while IFS= read -r ctx; do
  [ -z "$ctx" ] && continue
  # A matrix job's context is "Job name (matrix values)"; the declared job
  # `name:` is the part before the parenthesis.
  base="${ctx%% (*}"
  # A check's context is the job's `name:` when it has one, and otherwise the
  # job id itself (deny.yml relies on that — its job is `cargo-deny:` with no
  # `name:`). Match both, or the guard reports a false positive on the very
  # config it is meant to protect.
  owner=""
  for f in "$WF_DIR"/*.yml; do
    if grep -qE "^ *name: *['\"]?${base}" "$f" || grep -qE "^  ${base}:[[:space:]]*$" "$f"; then
      owner="$f"; break
    fi
  done

  if [ -z "$owner" ]; then
    printf '::error::required check "%s" matches no job name in %s/\n' "$ctx" "$WF_DIR"
    printf '  Either the check was renamed or its workflow was deleted; the queue\n'
    printf '  will wait for it forever.\n'
    fail=1
    continue
  fi

  if grep -q "merge_group" "$owner"; then
    printf '  OK   %-34s -> %s\n' "$ctx" "$(basename "$owner")"
  else
    printf '::error::required check "%s" lives in %s, which has no merge_group trigger\n' \
      "$ctx" "$(basename "$owner")"
    printf '  Add `merge_group:` to its `on:` block, or the merge queue will hang.\n'
    fail=1
  fi
done <<< "$contexts"

echo
if [ "$fail" -eq 0 ]; then
  echo "OK: every required check can report inside the merge queue"
else
  echo "::error::merge-queue triggers are inconsistent with the required checks"
fi
exit "$fail"
