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

LOG=/tmp/crate-test-$1.log

# ── Forensics beacon (hunt for the mcp-exec runner-death, issue #243) ──────
# The runner death destroys the in-progress step's log, every cgroup cap has
# failed to contain it, and the 15-min timeout below never fired — so the
# only way to see the box's final state is to exfiltrate it continuously.
# PATCH one issue comment every 30 s with the test-output tail + top
# processes + memory + D-state list. The last successful PATCH survives the
# runner's death.
#
# Armed BEFORE joining the cgroup so the beacon lives OUTSIDE the jail (an
# in-jail catastrophe can't take it down with the tests). `set +e` inside —
# the first beacon iteration of the previous revision died instantly because
# an inherited `set -e` aborted the subshell on the first failing collector.
BEACON_PID=""
if [ "${1:-}" = "xiaoguai-mcp-exec" ] && [ -n "${GH_TOKEN:-}" ]; then
  # POST-per-beat: the PATCH variant never landed a single update in two
  # rounds while the arming POST always worked — stop debugging the
  # difference, use the proven channel. Beat 0 at +10 s validates the
  # collectors; then every 120 s (≤ ~23 comments per 45-min hang).
  (
    set +e +o pipefail
    n=0
    sleep 10
    while true; do
      body="run ${GITHUB_RUN_ID:-?} beat #${n} $(date -u +%FT%TZ)
\`\`\`
—— test output tail ——
$(tail -c 3000 "$LOG" 2>/dev/null || echo none)
—— top rss ——
$(ps aux --sort=-rss 2>/dev/null | head -12 || true)
—— free ——
$(free -m 2>/dev/null || true)
—— testcap ——
mem.current=$(cat /sys/fs/cgroup/testcap/memory.current 2>/dev/null || echo '?') procs=$(wc -l </sys/fs/cgroup/testcap/cgroup.procs 2>/dev/null || echo '?')
—— D-state ——
$(ps -eo pid,stat,wchan:32,comm 2>/dev/null | awk '$2 ~ /D/' | head -8 || true)
—— dmesg tail ——
$(sudo dmesg 2>/dev/null | tail -5 || true)
\`\`\`"
      gh api -X POST "repos/${GITHUB_REPOSITORY}/issues/243/comments" \
        -f body="$body" >/dev/null 2>&1
      n=$((n + 1))
      sleep 120
    done
  ) &
  BEACON_PID=$!
fi

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

# timeout: every per-crate suite finishes in single-digit minutes when
# healthy; kill a hung suite at 15 min so the step fails GRACEFULLY.
# choom: prefer cargo as OOM victim over the runner agent.
# mold -run: memory-frugal linker without touching RUSTFLAGS (cache stays warm).
rc=0
timeout -k 30 900 choom -n 800 -- mold -run \
  cargo test -p "$1" --locked --jobs 1 2>&1 | tee "$LOG" || rc=$?

[ -n "$BEACON_PID" ] && kill "$BEACON_PID" 2>/dev/null || true
exit "$rc"
