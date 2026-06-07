#!/usr/bin/env bash
# CI test harness: run one crate's prebuilt test suite inside the memory-jail
# cgroup. (xiaoguai-mcp-exec does NOT go through this script — it is
# quarantined into its own job in rust.yml; hunt log in issue #243.)
#
# Why per-crate (one workflow step each): a runner death loses the
# IN-PROGRESS step's log, but completed steps' logs and step metadata
# survive — so the dying step names the culprit crate.
#
# Why nextest-from-archive: `cargo test -p <crate>` re-resolves features
# per-crate and silently RECOMPILED — and that rustc invocation
# intermittently spun forever (issue #243 beacon forensics). The archive is
# built once with workspace unification; here we only RUN. nextest's
# profile.ci terminate (.config/nextest.toml) kills and NAMES a hung test.
set -euo pipefail

if [ ! -f /sys/fs/cgroup/testcap/cgroup.procs ]; then
  sudo mkdir -p /sys/fs/cgroup/testcap
  echo 12G | sudo tee /sys/fs/cgroup/testcap/memory.max >/dev/null
  # swap.max=0: with ANY swap allowance a leaking test thrashes between RAM
  # and swap *within* its limits (D-state, SIGKILL pends, disk saturated,
  # runner starves). With zero swap the leak hits memory.max and the kernel
  # cgroup-OOM-kills the test binary immediately. The box's swap remains
  # available to the (un-jailed) build/link phase.
  echo 0 | sudo tee /sys/fs/cgroup/testcap/memory.swap.max >/dev/null
fi
echo $$ | sudo tee /sys/fs/cgroup/testcap/cgroup.procs >/dev/null

LOG=/tmp/crate-test-$1.log

# Exit-path hardening — the step must end even if a child becomes unkillable:
#  * ALL output goes to a FILE — an orphan can then only hold a file fd,
#    which blocks nothing (a pipe-holding orphan stalls the runner's
#    stream-EOF wait).
#  * Two timeout layers: the inner one (900s) kills the WORK; the outer one
#    (960s) kills the inner timeout — the WAITER — so bash always regains
#    control by 16 min.
# choom: prefer the test tree as OOM victim over the runner agent.
rc=0
timeout -k 10 960 bash -c "exec timeout -k 30 900 choom -n 800 -- \
  cargo nextest run --archive-file target/nextest-tests.tar.zst \
  --workspace-remap . -E 'package($1)' --profile ci --no-tests=pass" \
  > "$LOG" 2>&1 || rc=$?

echo "exit=$rc — last 100KB of test output:"
tail -c 100000 "$LOG" 2>/dev/null || echo "(no output captured)"
exit "$rc"
