# `deploy/`

Operational artifacts. The supported path is a single self-contained binary
with an embedded SQLite store (DEC-033 single-user pivot): `docker-compose.yml`
(one `xiaoguai-core` service), the `.deb`/`.rpm`/tarball, and the `systemd/`
unit. No Postgres, Valkey, or Redis.

Optional telemetry: layer `docker-compose.observability.yml` on top (opt-in;
the binary exposes `/metrics` + OTLP only when built with the `observability`
cargo feature, off by default).

> The multi-tenant-era Kubernetes/Helm/Istio/kustomize/Terraform and
> Postgres-HA artifacts were removed under the single-user pivot — see git
> history before the DEC-033 cutover if you need them.

## Quick bring-up

The simplest path needs no Docker and no toolchain — `pip install` drops the
native binary on PATH (macOS arm64/x86_64, Linux x86_64/aarch64):

```bash
pip install xiaoguai                 # PEP 668 systems (Debian 12 / Ubuntu 24):
                                     #   sudo apt install -y pipx && pipx install xiaoguai
xiaoguai serve                       # :7600, auto-creates ~/.xiaoguai/data.db

# in another terminal — first chat:
curl http://localhost:7600/healthz       # → ok
curl -X POST http://localhost:7600/v1/sessions \
  -H 'content-type: application/json' \
  -d '{"user_id":"usr_dev","model":"mock"}'
```

From a checkout with a Rust toolchain, `cargo run -p xiaoguai-cli -- serve`
does the same. Pre-built `.deb`/`.rpm`/tarball installs add the bundled web UI —
see the repo-root README's Quickstart. A no-network one-shot, no server needed:

```bash
xiaoguai chat --mock --prompt 'hello'
```

Self-check an install with `xiaoguai doctor` (database / providers / Ollama /
port), and keep the server running across reboots with
`xiaoguai service install` (systemd on Linux, launchd on macOS) — see
`docs/user-guide/install-and-verify.md`.

### Containerised (one command, full stack + web UI bundled)

```bash
docker compose -f deploy/docker-compose.yml up --build
```

Requires the Docker Compose **v2 plugin** (`docker compose version`); if that
errors with `unknown shorthand flag: 'f'`, install `docker-compose-plugin`.
The image bundles the web UI: open **http://localhost:7600/** for chat-ui and
**http://localhost:7600/admin/** for the admin console (served from
`/app/static` via `XIAOGUAI_SERVER__STATIC_DIR`).

## Layers

| Stage                                    | Lives where                             |
|------------------------------------------|------------------------------------------|
| Rust build                               | `Dockerfile` (rust:slim-bookworm)        |
| Runtime                                  | `Dockerfile` (distroless cc-debian12)    |
| Stack composition                        | `docker-compose.yml`                     |
| State                                    | named volume `xiaoguai_data` (embedded SQLite) |

## Bare-metal install (v1.1.6)

For deployments that don't run Docker, every `v*` tag publishes
bare-metal release tarballs:

```
xiaoguai-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
xiaoguai-vX.Y.Z-aarch64-unknown-linux-gnu.tar.gz
SHA256SUMS
```

Install on a systemd-based host (root required):

```bash
curl -LO https://github.com/xiaoguai-agent/xiaoguai/releases/download/vX.Y.Z/xiaoguai-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
tar -xzf xiaoguai-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
cd xiaoguai-vX.Y.Z-x86_64-unknown-linux-gnu
sudo bash scripts/install.sh

sudo cp /etc/xiaoguai/config.example.yaml /etc/xiaoguai/config.yaml
sudo $EDITOR /etc/xiaoguai/config.yaml      # set database / cache / hmac
sudo systemctl start xiaoguai-core
```

Build the tarballs locally:

```bash
# requires Rust + cross (https://github.com/cross-rs/cross) + Docker.
VERSION=1.1.6 bash scripts/release/build-tarball.sh
ls dist/
```

The systemd unit lives in `deploy/systemd/xiaoguai-core.service` —
`Type=simple` with full hardening (`ProtectSystem=strict`,
`NoNewPrivileges`, empty `CapabilityBoundingSet`, syscall filter,
read-only `/etc/xiaoguai`, etc.). Operators wanting `:80` / `:443`
add `CAP_NET_BIND_SERVICE` via a drop-in.

## What's not here (deferred post-v1.1.6)

- Helm chart with values + ingress + secrets refs.
- Debian / RPM packages (would need `cargo-deb` or `fpm`).
- musl statically-linked tarball (Alpine / older RHEL support).
- Windows MSI, macOS pkg.
