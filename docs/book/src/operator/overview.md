# Operator Guide

This guide covers day-2 operations for Xiaoguai deployments.

## Chapters

- [Day-2 Operations](day2.md) — migrations, HMAC key rotation, disaster recovery, observability
- [Systemd Hardening](systemd.md) — bare-metal production unit with capability drops and sandboxing
- [Security](security.md) — single-owner auth, audit chain, bandit scanning
- [Release Signing](release-signing.md) — cosign + SLSA attestations
- [Human-on-the-Loop Policy](human-on-the-loop.md) — budget-based escalation for agent actions (v1.2.3)
- [Outcome Telemetry](outcome-telemetry.md) — ROI attribution chains and dashboard queries (v1.2.4)

## Deployment paths

| Path | Best for |
|------|---------|
| **docker-compose** | Local evaluation; bundles the web UI |
| **Bare-metal tarball / `.deb` / `.rpm`** | VMs or bare-metal (systemd) |
| **pip wheel** | Python-centric environments, scripting, CI |

## Prerequisites

Under the single-user pivot (DEC-033) there are **no external datastores**.
State is one embedded SQLite file (created on first boot); the cache falls
back in-process. The only thing to front a public deployment with is:

- **TLS termination** — nginx, Caddy, or a cloud load balancer in front of
  `:7600`, plus a configured `auth.username`/`auth.password` (HTTP Basic).
