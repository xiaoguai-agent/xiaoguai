# syntax=docker/dockerfile:1.7
#
# Optimised multi-stage build for xiaoguai-core + xiaoguai CLI.
#
#   docker build -f deploy/Dockerfile -t xiaoguai:edge .
#
# Stage overview
# ──────────────
#   chef          – installs cargo-chef once; shared base for planner + builder
#   planner       – cargo chef prepare  (produces recipe.json from workspace)
#   frontend      – pnpm install + vite build for admin-ui + chat-ui
#   builder       – cargo chef cook (deps, cached) then cargo build --release
#   runtime       – distroless final image (~30 MB compressed)
#
# Cache behaviour
# ───────────────
#   • Cargo dependencies are cooked in a dedicated layer keyed only on
#     recipe.json (Cargo.lock). A source-only change hits the cook layer
#     cache and skips ~95 % of compile time.
#   • BuildKit cache mounts keep ~/.cargo/registry + /app/target between
#     invocations on the same host without polluting the image layers.
#   • Frontend deps are cached separately from frontend source code.
#
# Wave-3 crates verified in build:
#   xiaoguai-watch  xiaoguai-anomaly  xiaoguai-observability
#   xiaoguai-im-discord  xiaoguai-im-telegram
#   xiaoguai-im-mattermost  xiaoguai-im-slack
#   xiaoguai-orchestrator

ARG RUST_VERSION=1.88
ARG NODE_VERSION=20

# ── stage 1: install cargo-chef ──────────────────────────────────────────────
FROM rust:${RUST_VERSION}-slim-bookworm AS chef
WORKDIR /app

# OS build deps (pkg-config + libssl for sqlx/rustls; git for cargo deps that
# use git sources). Cached in its own layer — changes only when apt list bumps.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev ca-certificates git \
 && rm -rf /var/lib/apt/lists/*

RUN cargo install cargo-chef --locked

# ── stage 2: compute recipe ──────────────────────────────────────────────────
FROM chef AS planner
# Copy the full workspace so cargo-chef can walk every Cargo.toml.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN cargo chef prepare --recipe-path recipe.json

# ── stage 3: build frontend ──────────────────────────────────────────────────
FROM node:${NODE_VERSION}-alpine AS frontend
WORKDIR /app

# Install pnpm. Match the packageManager field in frontend/package.json.
RUN npm install -g pnpm@9.12.0

# Deps layer — busts only when lockfile or any workspace package.json changes.
# Copy ALL workspace package.json files BEFORE `pnpm install` so the install
# resolves devDependencies for every workspace package (admin-ui, chat-ui,
# shared). Sprint-11 (S11-2b) added a `build` script to shared/ that calls
# `tsc`; with the prior layout (only shared copied at install time) admin-ui
# and chat-ui ended up with no node_modules and the `tsc` binary was not on
# PATH at `pnpm -r build` time.
COPY frontend/pnpm-lock.yaml frontend/pnpm-workspace.yaml frontend/package.json ./
COPY frontend/shared/package.json    ./shared/package.json
COPY frontend/admin-ui/package.json  ./admin-ui/package.json
COPY frontend/chat-ui/package.json   ./chat-ui/package.json
COPY frontend/e2e/package.json       ./e2e/package.json
RUN pnpm install --frozen-lockfile

# Source layers — after deps so source changes don't re-download packages.
COPY frontend/shared    ./shared
COPY frontend/admin-ui  ./admin-ui
COPY frontend/chat-ui   ./chat-ui
RUN pnpm -r build

# ── stage 4: cook deps + compile binaries ────────────────────────────────────
FROM chef AS builder

# Cook dependencies — this layer is cache-stable as long as recipe.json
# (i.e. Cargo.lock + workspace member list) does not change.
COPY --from=planner /app/recipe.json recipe.json
RUN --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/root/.cargo/git \
    --mount=type=cache,target=/app/target \
    cargo chef cook --release --recipe-path recipe.json

# Now copy source — cache miss here only rebuilds changed crates, not deps.
COPY Cargo.toml Cargo.lock rust-toolchain.toml clippy.toml rustfmt.toml deny.toml ./
COPY crates ./crates
# Sprint-12 S12-0: `crates/xiaoguai-api/src/skills.rs` embeds the skill
# catalog via `include_str!("../../../catalog/skill_packs.json")` — must be
# present at compile time. Without this COPY the release-image workflow
# fails at `cargo build --release` with a file-not-found include_str error
# (regression first seen on the v1.8.1 release run).
COPY catalog ./catalog

RUN --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/root/.cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release \
        --bin xiaoguai-core \
        --bin xiaoguai \
 && cp target/release/xiaoguai-core /xiaoguai-core \
 && cp target/release/xiaoguai      /xiaoguai

# ── stage 5: minimal runtime image ───────────────────────────────────────────
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

# Distroless ships a non-root `nonroot` user (uid 65532). All state lives
# under /data which the operator should mount as a volume.
WORKDIR /app

# Rust binaries
COPY --from=builder /xiaoguai-core /usr/local/bin/xiaoguai-core
COPY --from=builder /xiaoguai      /usr/local/bin/xiaoguai

# Frontend static assets served by xiaoguai-core's embedded file server.
COPY --from=frontend /app/admin-ui/dist ./static/admin-ui
COPY --from=frontend /app/chat-ui/dist  ./static/chat-ui

# Defaults overridden via env (XIAOGUAI_*) or a mounted config.yaml.
ENV RUST_LOG=info,sqlx=warn

USER nonroot:nonroot
EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/xiaoguai-core"]
CMD ["serve"]
