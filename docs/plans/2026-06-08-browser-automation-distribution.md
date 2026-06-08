# Decision draft — P2 browser automation: the distribution problem

| | |
|---|---|
| Date | 2026-06-08 |
| Status | **Draft — awaiting owner go/no-go on distribution posture** |
| Scope | Resolve the ONE blocker gating P2 browser automation: a headless-browser dependency vs the single-binary / `pip install` distribution model (DEC-033). Implementation is deliberately out of scope until the posture below is chosen. |
| Related | [[coding-edition-roadmap]] (P2 entry), `deferred-big-features` memory, `crates/xiaoguai-coding` (the proven pattern) |

## 0. Why this doc exists

The *agent-loop* side of browser automation is **already a solved pattern** — it
mirrors `xiaoguai-coding`: a new governed crate + an in-process `McpClient`
registered into the agent `Toolbox`, so the loop auto HotL-gates `tool_call.*`,
audits, and checkpoints. The registration seam exists today
(`coding_bridge::build_coding_toolbox`, wired at `xiaoguai-core/src/lib.rs:323`).

What is **not** decided is everything below the tool layer: a real browser engine
(Chromium) is ~150–400 MB, cannot be compiled into the Rust binary, and cannot be
shipped inside a Python wheel. That collides head-on with the project's hard
constraint:

> **DEC-033:** 单二进制 — all capability compiled into one executable; `pip install
> xiaoguai` ships that binary per-platform.

This draft frames the collision honestly and lays out the options. **No code until
an owner picks a posture.**

## 1. The key reframe — is this even a DEC-033 violation?

Arguably **no**, and this is the crux. DEC-033 says *our* capabilities live in one
binary. It does not say the binary may never invoke a third-party program at
runtime — and in fact **xiaoguai already does exactly that**: the coding tools
shell out to the host's `git` and `gh` binaries (`xiaoguai-coding/src/git.rs`).
Those are external runtime dependencies the binary *detects and drives*, not
things compiled in.

A headless browser fits the same mental model: **Chromium as an optional runtime
resource, detected/launched by the single binary — not a distribution artifact.**
Under that framing the single-binary philosophy is intact; what changes is the
*first-run UX* (`pip install` no longer yields a fully self-contained tool — the
user must provision a browser, like they already must provision `git`/`gh` for
coding). That UX cost is the real thing to accept or reject — not a DEC-033 breach.

**Owner question 1:** Do you accept "Chromium as an optional, separately-provisioned
runtime dependency" (same class as `git`/`gh`), or is browser automation off the
table entirely under the single-binary product promise?

## 2. How the browser gets there — the options

If Q1 is "yes", the next axis is *how* a Chromium becomes available at runtime.
These are not mutually exclusive; the recommendation (§4) layers them.

### Option A — Detect a system browser (BYO Chromium)
Probe `PATH` / well-known locations for Chrome/Chromium/Edge; if absent, error with
a clear "install a browser or set `XIAOGUAI_BROWSER_PATH`" message.
- **Pros:** zero download, smallest footprint, mirrors the `git`/`gh` posture exactly.
- **Cons:** version skew (CDP protocol drift across Chrome versions); not every
  server host has a browser; "works on my machine" support load.

### Option B — Connect to a remote/user-managed CDP endpoint
xiaoguai is a pure CDP **client**: user runs their own
`chrome --remote-debugging-port` (or a container), passes
`XIAOGUAI_BROWSER_CDP_URL`. xiaoguai never manages a browser process.
- **Pros:** cleanest separation; no lifecycle/download code; great for the
  `xiaoguai serve` daemon model (browser runs beside it); trivially sandboxable.
- **Cons:** worst first-run UX for a casual user; pushes setup onto them.

### Option C — Auto-download a pinned Chromium on first use
On first browser-tool call, fetch a known-good Chromium build into
`~/.xiaoguai/chromium/<rev>` (the Puppeteer/Playwright model).
- **Pros:** best "it just works" UX; pinned revision ⇒ no CDP version skew.
- **Cons:** ~150 MB download + a downloader/cache/integrity-check to build and
  maintain; egress on first run (some environments forbid it); we own the
  update treadmill. **Needs research:** confirm whether the chosen Rust CDP crate
  (`chromiumoxide`?) ships a fetcher we can reuse, or we write one.

### Option D — Downgrade scope: no real browser
Skip a JS engine entirely; offer an HTTP-fetch + DOM-parse "read the web" tool
(static pages only, no click/JS/SPA).
- **Pros:** stays 100% single-binary, no new heavy dep, ships fast.
- **Cons:** not "browser automation" — can't drive SPAs, logins, or
  JS-rendered content. A different (smaller) product.

## 3. Cross-cutting concerns (apply to whichever option)

- **Security:** a headless browser is a large attack surface. Reuse the coding
  posture verbatim — every navigate/click/eval_js declares a `tool_call.<name>`
  scope so the loop HotL-gates it; egress-ish actions behind an opt-in flag
  mirroring `XIAOGUAI_CODING_ALLOW_EGRESS`; `eval_js` is the most dangerous tool
  and should be gated hardest (or omitted from the MVP).
- **Opt-in by default:** like coding tools, register the browser toolbox only when
  an explicit env (e.g. `XIAOGUAI_BROWSER_ENABLED`) is set — never auto-on.
- **Daemon-resident:** like /loop and HotL, this belongs under `xiaoguai serve`,
  not a one-shot CLI invocation (a browser session is stateful).
- **CDP crate choice is itself a decision** (`chromiumoxide` vs `headless_chrome`
  vs a thin hand-rolled CDP client). Each adds a non-trivial dep tree — run it
  past `cargo-deny`/`cargo-vet` before committing. **Not decided here.**

## 4. Recommendation

**Layered, BYO-first:**
1. Ship **Option B (remote CDP URL)** + **Option A (detect system browser)** first —
   together they need *no* downloader and fit the `git`/`gh` precedent and the
   `serve` daemon model. Opt-in via `XIAOGUAI_BROWSER_ENABLED`; browser provided
   via `XIAOGUAI_BROWSER_CDP_URL` or `XIAOGUAI_BROWSER_PATH`.
2. Treat **Option C (auto-download)** as a *later convenience layer*, only if
   first-run UX demands it — it carries the most code and maintenance.
3. Keep **Option D** in pocket as the fallback if Q1 is "no real browser" — it's a
   genuinely different, lighter feature, not a degraded version of this one.

This keeps the binary single, adds Chromium only as a detected/remote runtime
resource (DEC-033-consistent per §1), reuses the entire coding governance pattern,
and defers the expensive downloader until proven necessary.

## 5. Decisions needed from the owner (blocking)

1. **Posture:** accept Chromium as an optional runtime dependency (like `git`/`gh`),
   or rule out real-browser automation under the single-binary promise?
2. **Provisioning default** (if §1=yes): BYO/detect (A+B) first, or invest in
   auto-download (C) up front?
3. **CDP crate:** willing to take `chromiumoxide`/`headless_chrome`'s dep tree
   through `cargo-deny`/`cargo-vet`, or prefer a thin hand-rolled CDP client?
4. **MVP tool surface:** navigate/read_dom/screenshot only (safe), or include
   click/type/eval_js (powerful, larger HotL surface) from day one?

Once these are answered, the implementation plan (mirror `xiaoguai-coding`: crate
skeleton → governed tools → `McpClient` → core bridge → CLI, each step
clippy+nextest green) is straightforward and can be written as a follow-up.
