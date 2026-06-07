#!/usr/bin/env bash
# CI test harness: run one crate's test suite inside the memory-jail cgroup.
#
# Why per-crate (one workflow step each): the recurring "runner lost
# communication" death loses the IN-PROGRESS step's log entirely, but
# completed steps' logs and the step metadata survive — so with one step per
# crate, the dying step itself names the culprit crate even when nothing else
# survives.
#
# Why the cgroup: a ballooning test must OOM inside the jail (one binary dies,
# step fails gracefully, logs + cache save) instead of taking the box down.
set -euo pipefail

if [ ! -f /sys/fs/cgroup/testcap/cgroup.procs ]; then
  sudo mkdir -p /sys/fs/cgroup/testcap
  echo 12G | sudo tee /sys/fs/cgroup/testcap/memory.max >/dev/null
  # swap.max=0 is the load-bearing line: with ANY swap allowance a leaking
  # test thrashes between RAM and swap *within* its limits — processes hang
  # in D-state (even SIGKILL pends), the thrash saturates disk I/O, and the
  # runner agent outside the cgroup starves ("lost communication"). With
  # zero swap the leak hits memory.max and the kernel cgroup-OOM-kills the
  # test binary immediately. The box's swap remains available to the
  # (un-jailed) build/link phase.
  echo 0 | sudo tee /sys/fs/cgroup/testcap/memory.swap.max >/dev/null
fi
echo $$ | sudo tee /sys/fs/cgroup/testcap/cgroup.procs >/dev/null

LOG=/tmp/crate-test-$1.log

# ── Forensics beacon (hunt for the mcp-exec runner-death, issue #243) ──────
# The runner death destroys the in-progress step's log, every cgroup cap has
# failed to contain it, and the 15-min timeout below did not fire — so the
# only way to see the box's final state is to exfiltrate it continuously.
# While the suspect crate's tests run, PATCH one issue comment every 60 s
# with the test-output tail + top processes + memory. The last successful
# PATCH survives the runner's death.
BEACON_PID=""
if [ "${1:-}" = "xiaoguai-mcp-exec" ] && [ -n "${GH_TOKEN:-}" ]; then
  cid=$(gh api -X POST "repos/${GITHUB_REPOSITORY}/issues/243/comments" \
    -f body="beacon armed: run ${GITHUB_RUN_ID:-?} $(date -u +%FT%TZ)" \
    --jq .id 2>/dev/null || true)
  if [ -n "$cid" ]; then
    (
      while true; do
        sleep 60
        body="run ${GITHUB_RUN_ID:-?} beat $(date -u +%FT%TZ)
\`\`\`
—— test output tail ——
$(tail -c 3000 "$LOG" 2>/dev/null || echo none)
—— top rss ——
$(ps aux --sort=-rss 2>/dev/null | head -12)
—— free ——
$(free -m 2>/dev/null)
—— testcap ——
mem.current=$(cat /sys/fs/cgroup/testcap/memory.current 2>/dev/null) peak=$(cat /sys/fs/cgroup/testcap/memory.peak 2>/dev/null)
—— D-state ——
$(ps -eo pid,stat,wchan:32,comm 2>/dev/null | awk '$2 ~ /D/' | head -8)
\`\`\`"
        gh api -X PATCH "repos/${GITHUB_REPOSITORY}/issues/comments/${cid}" \
          -f body="$body" >/dev/null 2>&1 || true
      done
    ) &
    BEACON_PID=$!
  fi
fi

# timeout: every per-crate suite finishes in single-digit minutes when
# healthy; kill a hung suite at 15 min so the step fails GRACEFULLY.
# choom: prefer cargo as OOM victim over the runner agent.
# mold -run: memory-frugal linker without touching RUSTFLAGS (cache stays warm).
rc=0
timeout -k 30 900 choom -n 800 -- mold -run \
  cargo test -p "$1" --locked --jobs 1 2>&1 | tee "$LOG" || rc=$?

[ -n "$BEACON_PID" ] && kill "$BEACON_PID" 2>/dev/null || true
exit "$rc"
