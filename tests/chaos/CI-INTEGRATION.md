# CI Integration for Chaos Scenarios

## Why These CANNOT Run in Standard CI

These chaos scenarios require capabilities that are incompatible with standard
CI runners (GitHub Actions default, GitLab shared runners, etc.):

| Requirement | Standard CI | Why Needed |
|-------------|:-----------:|-----------|
| Privileged Docker socket | Not available | `docker compose stop`, `docker update` |
| `CAP_NET_ADMIN` on containers | Not available | `tc netem` packet loss injection |
| `CAP_SYS_TIME` on containers | Not available | `clock-skew` date manipulation |
| cgroup v2 blkio write access | Not available | `slow-disk` I/O throttle |
| Long-running tests (60-120s) | Timeout risk | OOM + recovery windows |
| Shared staging environment | Conflicts with dev | Chaos degrades the environment |

Attempting to run these in standard CI will result in:
- Silent no-ops (scripts detect missing docker and exit 0)
- False positives if network injection falls back silently
- Noisy logs cluttering PR checks

## Recommended: Monthly Staging Run via Dedicated Runner

### Option A: Self-Hosted Runner with Privileged Docker Access

Add a GitHub Actions workflow triggered on `schedule` (monthly) or
`workflow_dispatch` (manual game-day):

```yaml
# .github/workflows/chaos-monthly.yml
name: Chaos Engineering — Monthly Game-Day

on:
  schedule:
    - cron: '0 2 1 * *'   # 1st of each month at 02:00 UTC
  workflow_dispatch:
    inputs:
      scenario:
        description: 'Scenario to run (leave blank for all)'
        required: false
        default: 'all'

jobs:
  chaos:
    runs-on: self-hosted-privileged   # dedicated runner with docker socket
    timeout-minutes: 30
    environment: staging-chaos         # separate environment with approvals

    steps:
      - uses: actions/checkout@v4

      - name: Start compose stack
        run: docker compose -f deploy/docker-compose.yml up -d --wait

      - name: Run chaos scenarios
        run: |
          SCENARIO="${{ github.event.inputs.scenario }}"
          if [[ "$SCENARIO" == "all" || -z "$SCENARIO" ]]; then
            for script in tests/chaos/kill-pg.sh tests/chaos/kill-redis.sh \
                          tests/chaos/kill-otel.sh tests/chaos/network-partition-pg.sh \
                          tests/chaos/oom-xiaoguai-core.sh tests/chaos/clock-skew.sh \
                          tests/chaos/slow-disk.sh; do
              echo "=== Running $script ==="
              bash "$script" --restore-on-error || echo "SCENARIO FAILED: $script (exit $?)"
            done
          else
            bash "tests/chaos/${SCENARIO}.sh" --restore-on-error
          fi

      - name: Collect logs
        if: always()
        run: |
          docker compose -f deploy/docker-compose.yml logs > /tmp/compose-logs.txt
          ls /tmp/chaos-*.log 2>/dev/null && cat /tmp/chaos-*.log || true

      - name: Upload logs as artifact
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: chaos-logs-${{ github.run_id }}
          path: |
            /tmp/chaos-*.log
            /tmp/compose-logs.txt

      - name: Teardown
        if: always()
        run: docker compose -f deploy/docker-compose.yml down -v
```

### Option B: Syntax-Only Check in Standard CI

To validate script syntax on every PR without executing chaos:

```yaml
# .github/workflows/chaos-syntax.yml
name: Chaos Scripts — Syntax Check

on: [pull_request]

jobs:
  syntax:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: bash -n syntax check
        run: |
          for script in tests/chaos/*.sh; do
            echo "Checking: $script"
            bash -n "$script"
          done
      - name: dry-run check (no docker required)
        run: |
          for script in tests/chaos/*.sh; do
            echo "Dry-running: $script"
            bash "$script" --dry-run
          done
```

This is the ONLY chaos CI that runs on standard shared runners. The dry-run
mode detects missing docker and exits 0, verifying logic paths without side effects.

## Runner Requirements for Full Chaos Runs

The self-hosted runner must have:

```yaml
# docker-compose for runner (or equivalent host config)
services:
  runner:
    image: myorg/github-runner:ubuntu-22.04
    privileged: true                          # required for CAP_NET_ADMIN, CAP_SYS_TIME
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock
      - /sys/fs/cgroup:/sys/fs/cgroup:rw      # required for blkio throttle
    security_opt:
      - apparmor:unconfined
```

## Tagging Chaos-Capable Containers

To enable `network-partition-pg` and `clock-skew`, the deploy compose must
add capabilities to the relevant services. For staging only:

```yaml
# deploy/docker-compose.chaos.yml (staging override)
services:
  postgres:
    cap_add:
      - NET_ADMIN    # for tc netem packet loss injection
  xiaoguai-core:
    cap_add:
      - SYS_TIME     # for clock-skew test
```

Run with:
```bash
docker compose -f deploy/docker-compose.yml \
               -f deploy/docker-compose.chaos.yml \
               up -d --wait
```

## Failure Notification

Configure the workflow to notify on failure:

```yaml
- name: Notify on failure
  if: failure()
  uses: slackapi/slack-github-action@v1
  with:
    payload: |
      {"text": "Chaos game-day FAILED on ${{ github.ref }} — check artifacts"}
  env:
    SLACK_WEBHOOK_URL: ${{ secrets.CHAOS_SLACK_WEBHOOK }}
```

## Frequency Recommendation

| Scenario Group | Frequency | Trigger |
|----------------|-----------|---------|
| All 7 scenarios | Quarterly | Game-day (manual) |
| `kill-pg` + `kill-redis` | Monthly | Automated staging |
| `oom-xiaoguai-core` | Post-deploy | After container config changes |
| `network-partition-pg` | Post-deploy | After DB topology changes |
| `clock-skew` | Quarterly | JWT / auth library upgrades |
| Syntax check (`--dry-run`) | Every PR | Standard CI |
