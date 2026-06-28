# Xiaoguai Floater

A lightweight, always-on-top **floating chat window** for macOS (Tauri v2). Press
a global hotkey, a small chat window pops up, you talk to your local
`xiaoguai serve`, and it tucks away when you click elsewhere or press `Esc`.

It is a **thin client**: it does not embed a model or a server. It connects to a
running `xiaoguai serve` over HTTP (default `http://localhost:7600`) and uses the
same session + streaming API as the web chat UI. Nothing about the
single-binary server (DEC-033) changes.

```
┌──────────────────────────────┐
│  小怪 · Floater      思考中…  │   ← undecorated, draggable title bar
├──────────────────────────────┤
│  you: 你好                    │
│  小怪: 嗨，我能帮你…            │   ← streamed token-by-token (SSE)
├──────────────────────────────┤
│  问点什么…  (Enter 发送 · Esc) │   ← composer
└──────────────────────────────┘
```

## Behaviour

| Action | Result |
| --- | --- |
| `Alt + Space` (global) | Toggle the window: show + focus the input, or hide if already focused. |
| Type + `Enter` | Send the message; the reply streams in live. |
| `Shift + Enter` | Newline in the composer. |
| `Esc` | Hide the window. |
| Click another app (blur) | Auto-hide. |

The window **starts hidden** — summon it with the hotkey. It keeps its
conversation while alive, so re-summoning is instant.

## Prerequisites

1. **A running serve.** In the repo root:
   ```bash
   cargo run -p xiaoguai-cli -- serve          # binds 127.0.0.1:7600
   # or, if installed:  xiaoguai serve
   ```
   Make sure at least one LLM provider is configured (see the main README), or
   the agent has nothing to answer with.

2. **Tauri v2 system dependencies + Rust** (for building the desktop app):
   - **Rust** (stable; this repo pins 1.93.0 via `rust-toolchain.toml`).
   - **macOS**: Xcode Command Line Tools (`xcode-select --install`). The system
     WebKit web view is used — no extra runtime to ship.
   - See <https://v2.tauri.app/start/prerequisites/> for the full list.

3. **Node + pnpm** (this project uses pnpm; Node ≥ 18).

## Run it (dev)

This is a **standalone pnpm project** (its own `node_modules`/lockfile — it is
*not* part of the `frontend/` pnpm workspace, so it can't affect the
`chat-ui` / `admin-ui` builds).

```bash
cd desktop
pnpm install            # JS deps (incl. a local @tauri-apps/cli)
pnpm tauri dev          # compiles the Rust app + serves the UI, opens the window
```

`pnpm tauri dev` runs `vite` (the UI on :1430) and the Rust app together. The
window appears hidden — press **Alt+Space** to summon it.

> The very first `pnpm tauri dev` compiles Tauri's native dependencies and can
> take a few minutes. Subsequent runs are fast.

## Build a `.app` / `.dmg`

```bash
cd desktop
pnpm tauri build
```

Outputs land under `src-tauri/target/release/bundle/`:
- `macos/Xiaoguai Floater.app`
- `dmg/Xiaoguai Floater_0.1.0_<arch>.dmg`

(The bundle targets are `app` and `dmg`, set in `src-tauri/tauri.conf.json`.)

> Unsigned builds: macOS Gatekeeper will quarantine an unsigned `.app`. For
> local use, right-click → Open the first time, or run
> `xattr -dr com.apple.quarantine "Xiaoguai Floater.app"`. Code-signing /
> notarization is out of scope for this MVP.

## Pointing at a different serve / authentication

The serve runs **open (no auth) on localhost by default**, so the floater works
out of the box. If you protect the serve with the single-owner HTTP Basic
credential, or run it on another host/port, configure the floater via
environment variables (read once at startup):

| Variable | Default | Meaning |
| --- | --- | --- |
| `XIAOGUAI_FLOATER_URL` | `http://localhost:7600` | Serve base URL. |
| `XIAOGUAI_FLOATER_USER` + `XIAOGUAI_FLOATER_PASS` | — | HTTP Basic owner credentials. |
| `XIAOGUAI_FLOATER_TOKEN` | — | A Bearer token (takes precedence over Basic). |

Example:

```bash
XIAOGUAI_FLOATER_URL=http://localhost:7600 \
XIAOGUAI_FLOATER_USER=owner \
XIAOGUAI_FLOATER_PASS=hunter2 \
pnpm tauri dev
```

For a packaged `.app`, set the variables in the launching shell, or wrap the
binary in a launcher that exports them.

> **Why env vars (not webview `fetch`)?** All HTTP is issued from the Rust side
> via `reqwest`, not from the web view. A Tauri web view's origin is
> `tauri://localhost`, which is **not** a loopback origin, so the serve's CORS
> policy would block a web-view `fetch` to `:7600`. Issuing the request from
> Rust sidesteps CORS entirely and lets us stream Server-Sent Events cleanly
> back into the UI as Tauri events. (If you ever wanted web-view `fetch`
> instead, you'd have to start the serve with
> `XIAOGUAI_CORS_ALLOWED_ORIGINS=tauri://localhost`.)

## How it talks to the serve

The wire contract mirrors the web chat UI exactly (see
`crates/xiaoguai-api/src/routes/sessions.rs` and `src/sse.rs`):

1. **Create a session** (lazily, on the first message):
   `POST /v1/sessions` with `{ "user_id": "floater", "model": "" }`
   → `{ "id": "...", ... }`. An empty `model` lets the server pick its default
   provider/model.

2. **Send + stream**:
   `POST /v1/sessions/{id}/messages` with `{ "content": "..." }`, `Accept:
   text/event-stream`. The response is an SSE stream; each frame is

   ```
   event: text_delta
   data: {"type":"text_delta","delta":"嗨"}
   id: 1

   ```

   The Rust side parses each frame's `data:` JSON and forwards it to the UI over
   the `chat://event` Tauri channel. Recognised event `type`s: `text_delta`,
   `tool_call_started`, `tool_call_finished`, `iteration_completed`, `done`
   (carries `stop_reason`), `error` (carries `message`), `hotl_pending`,
   `hotl_resolved`.

## Project layout

```
desktop/
├── README.md                 # this file
├── package.json              # standalone pnpm project (Vite + Tauri CLI)
├── tsconfig.json
├── vite.config.ts            # dev server on :1430, build → dist/
├── index.html                # the floating card markup
├── src/                      # frontend (vanilla TypeScript, no framework)
│   ├── main.ts               #   UI controller: events, keybinds, streaming
│   ├── chat.ts               #   Tauri bridge: invoke commands + listen
│   ├── view.ts               #   DOM rendering helpers (bubbles, status)
│   ├── types.ts              #   AgentEvent / ChatFrame wire types
│   └── styles.css            #   floating-card styles (light + dark)
└── src-tauri/                # Rust (Tauri v2) — STANDALONE cargo project
    ├── Cargo.toml            #   own [workspace] → not in the root 38-crate ws
    ├── build.rs
    ├── tauri.conf.json       #   window (640×460, undecorated, always-on-top,
    │                         #   transparent, skip-taskbar, hidden) + mac bundle
    ├── capabilities/
    │   └── default.json      #   core event/window permissions for the webview
    ├── icons/                #   app icons (.icns/.ico/.png)
    └── src/
        ├── main.rs           #   binary entrypoint → lib::run()
        ├── lib.rs            #   app wiring: Alt+Space shortcut, blur→hide, cmds
        ├── window.rs         #   show / hide / toggle helpers
        ├── serve_client.rs   #   reqwest HTTP + SSE parsing → chat://event
        └── config.rs         #   env-driven URL + auth resolution
```

## Tests

The Rust SSE-parsing and config logic have unit tests:

```bash
cd desktop/src-tauri
cargo test
```
