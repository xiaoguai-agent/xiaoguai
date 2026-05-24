# Runbook: systemd hardening — Type=notify + WatchdogSec + CAP_NET_BIND

Procedures for the v1.1.6.2 systemd integration additions.

---

## 1. Verify Type=notify works

### Pre-flight check

```bash
# Check the binary version supports sd-notify (v1.1.6.2+):
xiaoguai-core --version

# Static analysis — systemd-analyze catches unknown directives and
# conflicting settings before you restart the service.
systemd-analyze verify /etc/systemd/system/xiaoguai-core.service
# Expected: no output (clean).
```

### Watch the startup handshake

```bash
# In one terminal, follow the journal:
journalctl -u xiaoguai-core -f

# In another terminal, restart the service:
sudo systemctl restart xiaoguai-core

# Expected journal sequence (condensed):
#   xiaoguai-core[PID]: serve: connecting to Postgres
#   xiaoguai-core[PID]: serve: pg ok
#   xiaoguai-core[PID]: serve: api listening local=0.0.0.0:7600
#   systemd[1]: xiaoguai-core.service: Got notification message from PID NNN (READY=1)
#   systemd[1]: Started Xiaoguai Core API server.
```

If `Type=simple` was previously set, the "Got notification message" line
will not appear and systemd declares the unit active immediately on fork.
With `Type=notify` systemd waits for the `READY=1` datagram before
proceeding to any `After=` / `Before=` ordered units.

### Confirm active state

```bash
systemctl is-active xiaoguai-core
# → active

systemctl show xiaoguai-core --property=ActiveEnterTimestamp,ActiveEnterTimestampMonotonic
# ActiveEnterTimestamp should be after the startup log lines above,
# not before them (which would be the case with Type=simple).
```

### What if READY=1 never arrives?

Symptoms: `systemctl status xiaoguai-core` shows `activating (start)` for
longer than `TimeoutStartSec` (default 90 s), then the unit fails.

Causes and fixes:

| Cause | Fix |
|-------|-----|
| Binary older than v1.1.6.2 | Upgrade or change back to `Type=simple` |
| `NOTIFY_SOCKET` not set by systemd | Ensure `Type=notify` in the `[Service]` block; check there is no override dropping the env var |
| Binary panics before the `notify_ready()` call | Check journal for a Rust panic traceback; fix the underlying crash |
| `ProtectSystem=strict` blocks socket access | Should not happen — systemd owns `NOTIFY_SOCKET` and grants access automatically; if you see EACCES, check SELinux/AppArmor policy |

---

## 2. Enable WatchdogSec

The watchdog is **opt-in**. The base unit has `# WatchdogSec=30s` commented out.

### Enable

```bash
# Option A: edit the unit file directly (not recommended — upgrades overwrite)
sudo systemctl edit xiaoguai-core
# Add under [Service]:
#   WatchdogSec=30s

# Option B: drop-in (recommended):
sudo mkdir -p /etc/systemd/system/xiaoguai-core.service.d
cat <<'EOF' | sudo tee /etc/systemd/system/xiaoguai-core.service.d/50-watchdog.conf
[Service]
WatchdogSec=30s
EOF
sudo systemctl daemon-reload
sudo systemctl restart xiaoguai-core
```

### Verify pings are landing

```bash
journalctl -u xiaoguai-core | grep -i watchdog
# Expected:
#   sd_notify: watchdog enabled — spawning ping task watchdog_usec=30000000 ping_interval_ms=15000
```

```bash
# After 30+ seconds, the service should still be active:
systemctl is-active xiaoguai-core
# → active
```

### Tune the interval

Set `WatchdogSec=` to at least 2× the expected worst-case event-loop pause
(GC, heavy query, etc.). A 30 s watchdog with 15 s pings is conservative
and safe for most deployments. Tighten to 10 s only on well-profiled systems.

### Disable the watchdog

```bash
sudo rm /etc/systemd/system/xiaoguai-core.service.d/50-watchdog.conf
sudo systemctl daemon-reload
sudo systemctl restart xiaoguai-core
```

---

## 3. Opt-in CAP_NET_BIND_SERVICE drop-in for :80 / :443

By default the base unit runs with an empty capability set. This drop-in
grants only the one capability needed to bind to privileged ports.

### Install

```bash
sudo cp deploy/systemd/xiaoguai-core.service.d/10-low-port.conf \
    /etc/systemd/system/xiaoguai-core.service.d/10-low-port.conf
sudo systemctl daemon-reload
```

Update the config to use the desired port:

```bash
# Edit /etc/xiaoguai/config.yaml:
#   server:
#     port: 443
# Or pass via environment drop-in — see README.md in the service.d dir.
```

Restart and verify:

```bash
sudo systemctl restart xiaoguai-core
systemctl show xiaoguai-core | grep -E 'AmbientCapabilities|CapabilityBounding'
# AmbientCapabilities=CAP_NET_BIND_SERVICE
# CapabilityBoundingSet=CAP_NET_BIND_SERVICE
```

### Verify binding

```bash
ss -tlnp | grep xiaoguai
# Should show LISTEN on *:443 (or :80).
```

### Revert

```bash
sudo rm /etc/systemd/system/xiaoguai-core.service.d/10-low-port.conf
sudo systemctl daemon-reload && sudo systemctl restart xiaoguai-core
# Remember to also change the port back in config.
```

---

## 4. Quick reference

```bash
# Static verification (catches syntax errors before restart):
systemd-analyze verify /etc/systemd/system/xiaoguai-core.service

# Show active notify/watchdog/capability settings:
systemctl show xiaoguai-core --property=Type,WatchdogUSec,AmbientCapabilities,CapabilityBoundingSet

# Follow startup handshake live:
journalctl -u xiaoguai-core -f --since now

# List all active drop-ins:
systemctl cat xiaoguai-core | head -5
# The header shows the base file + any drop-ins merged.
```
