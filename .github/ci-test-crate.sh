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
  # swap.max=0 is the load-bearing line: with ANY swap allowance a leaking
  # test thrashes between RAM and swap *within* its limits — processes hang
  # in D-state (even SIGKILL pends; the 15-min timeout below proved unable
  # to fire), the thrash saturates disk I/O, and the runner agent outside
  # the cgroup starves ("lost communication"). With zero swap the leak hits
  # memory.max and the kernel cgroup-OOM-kills the test binary immediately:
  # graceful step failure, logs survive, dmesg names the victim. The box's
  # 16G swap remains available to the (un-jailed) build/link phase.
  echo 0 | sudo tee /sys/fs/cgroup/testcap/memory.swap.max >/dev/null
fi
echo $$ | sudo tee /sys/fs/cgroup/testcap/cgroup.procs >/dev/null

# timeout: every per-crate suite finishes in single-digit minutes when
# healthy. A HUNG test otherwise leaks until the box dies (the
# xiaoguai-mcp-exec ~45 min runner-death signature, 2026-06-07): kill the
# whole suite at 15 min so the step fails GRACEFULLY — logs survive and
# cargo's output names the hung test.
# choom: prefer cargo as OOM victim over the runner agent.
# mold -run: memory-frugal linker without touching RUSTFLAGS (cache stays warm).
exec timeout -k 30 900 choom -n 800 -- mold -run cargo test -p "$1" --locked --jobs 1
