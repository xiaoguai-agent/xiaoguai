# xiaoguai as a deployable node — integration contracts & roadmap

xiaoguai is a **single-binary, single-owner, embedded-SQLite agent** (DEC-033).
It is built to run as one self-contained, governed **node** — not as a
multi-tenant control plane. When a larger system (an orchestrator, a deployment
script, CI, or a fleet manager) provisions and drives xiaoguai nodes, it relies
on the integration contracts below.

Anything that implies more than one owner sharing a control plane —
multi-tenant identity and role models, cross-node routing, fleet-wide billing,
a shared artifact store — lives in that *outer* system. xiaoguai **integrates
with** such a system as a node (§1–§3); it does not implement it (see
[Out of scope](#out-of-scope)).

---

## 1. Headless provisioning (no interactive UI)

A node can be stood up and configured with zero interactive steps, so an
orchestrator can template it:

- **Install** — `pip install xiaoguai`, a `.deb` / `.rpm` / tarball, or
  `scripts/quickstart-linux.sh` (downloads the tarball, runs the wizard, serves).
- **Configure** — a YAML file (`~/.xiaoguai/config.yaml`) *or* environment
  variables: `XIAOGUAI_SERVER__HOST` / `XIAOGUAI_SERVER__PORT`,
  `DATABASE_URL`, `XIAOGUAI_AUTH__USERNAME` / `XIAOGUAI_AUTH__PASSWORD`,
  `XIAOGUAI_AUDIT_SIGNING_KEY`, `XIAOGUAI_AT_REST_KEY`. Zero-config defaults
  (`:7600`, `~/.xiaoguai/data.db`) mean a clean box serves immediately.
- **Inject a provider key without a TTY** — `xiaoguai provider register
  --api-key-stdin` (key read from stdin, never argv) or the `xiaoguai init`
  wizard. The admin web UI's Providers pane is the interactive equivalent.
- **Run** — `xiaoguai serve [--host --port]`. A non-loopback bind requires
  owner auth (SEC-01: set the `XIAOGUAI_AUTH__*` pair).

**Status: ✅ shipped.** Roadmap: a one-page "headless deployment contract"
section in the docs so an orchestrator integrator has a single reference.

## 2. Routing through an upstream model gateway

To send all model traffic through an external governance / billing gateway,
register a provider of kind `openai_compat` whose `endpoint` is the gateway's
base URL and whose `api_key` is the gateway-issued key. xiaoguai then routes
every turn through that single upstream; quota, billing, and observability
happen at the gateway. **Configuration only — no code or rebuild.**

**Status: ✅ supported** by the existing provider registry + router. Roadmap: a
short "point a node at your gateway" note + example.

## 3. Chat-platform (IM) integration

xiaoguai ships adapters for **Feishu, DingTalk, WeCom, Slack, Telegram,
Discord, and Mattermost** via an IM gateway mounted into `serve`. Each verifies
inbound webhooks (signature / secret) and drives the **same governed agent
loop** — Human-in-the-Loop gating + audit — as the HTTP and CLI surfaces.

**Status: ✅ built + wired into `serve`.** Roadmap: a live end-to-end smoke
(real provider app → webhook → reply) and a short "connect your chat platform"
guide (Feishu first).

## 4. Branding

Optional white-label / logo neutralization of the bundled web UI.

**Status: roadmap (low priority).**

---

## Out of scope

These belong in the orchestrating system, not in xiaoguai — implementing them
here would break the single-owner / single-binary contract (DEC-033):

- multi-tenant identity, role, and environment models (more than one owner
  sharing a control plane);
- a shared model-gateway (key issuance, cross-tenant routing or billing);
- a shared artifact / package store;
- a front-door reverse proxy.

xiaoguai's job is to be a clean node that such a system can **deploy (§1),
point at a gateway (§2), and reach over a chat platform (§3)**.
