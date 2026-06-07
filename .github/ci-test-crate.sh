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
# step fails gracefully, logs + cache save) instead of swap-thrashing the
# whole box until the runner agent misses heartbeats.
set -euo pipefail

if [ ! -f /sys/fs/cgroup/testcap/cgroup.procs ]; then
  sudo mkdir -p /sys/fs/cgroup/testcap
  echo 12G | sudo tee /sys/fs/cgroup/testcap/memory.max >/dev/null
  # swap controller may be absent — cap is best-effort
  echo 4G | sudo tee /sys/fs/cgroup/testcap/memory.swap.max >/dev/null || true
fi
echo $$ | sudo tee /sys/fs/cgroup/testcap/cgroup.procs >/dev/null

# choom: prefer cargo as OOM victim over the runner agent.
# mold -run: memory-frugal linker without touching RUSTFLAGS (cache stays warm).
exec choom -n 800 -- mold -run cargo test -p "$1" --locked --jobs 1
