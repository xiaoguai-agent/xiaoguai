# Operator Guide

This guide covers day-2 operations for Xiaoguai deployments.

## Chapters

- [Day-2 Operations](day2.md) — migrations, HMAC key rotation, disaster recovery, observability
- [High Availability](ha.md) — PG logical replication + Valkey Cluster + nginx topology (v1.1.4)
- [Systemd Hardening](systemd.md) — bare-metal production unit with capability drops and sandboxing
- [Security](security.md) — OIDC, RBAC, RLS, audit chain, bandit scanning
- [Release Signing](release-signing.md) — cosign + SLSA attestations

## Deployment paths

| Path | Best for |
|------|---------|
| **docker-compose** | Local development and single-node evaluation |
| **Helm chart** | Kubernetes production deployments |
| **Bare-metal tarball** | VMs or bare-metal without container runtime |
| **pip wheel** | Python-centric environments, scripting, CI |

## Prerequisites

All paths share the same external dependencies:

- **Postgres 16** (or compatible) with `wal_level = logical` if HA is required
- **Valkey 8** (or Redis 7.x) — not Redis 7.4+ (SSPL license incompatibility)
- **TLS termination** — nginx, Caddy, or cloud load balancer in front of `:8080`
