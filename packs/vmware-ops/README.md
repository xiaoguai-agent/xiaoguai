# VMware Ops Skill Pack

Vertical ops agent-team for VMware infrastructure. It **consumes the
vmware-skill family over MCP** (9 read-only diagnostic servers + vmware-pilot
for execution) — it reimplements nothing. Every tool call lands on a
vmware-skill MCP server whose own `@vmware_tool` layer carries audit /
sanitize / dry-run / rollback.

## Why this is a pack (not a platform product)

This replaces the removed agent-platform **C22 "Agent Teams 智能运维总控"**.
Operating VMware = an agent calling vmware-skill tools; orchestration is a
generic agent capability. So the ops agent-team belongs at the **agent layer
(xiaoguai)** as a pack, not as an agent-platform product line. Decision +
判据: agent-platform `docs/architecture/38-2026-06-11-c22-removal-ops-agent-teams-to-xiaoguai.md`.

## What's in it

| File | Role |
|---|---|
| `agents/advice-team.yaml` | 9 conversation-only domain personas (vSphere / monitoring / storage / VKS / NSX / NSX-security / Aria / AVI / hardening). Diagnoses & recommends; never executes. |
| `agents/ops-triage.yaml` | Incident → RCA. Read-only context from vmware-aiops/monitor/aria, drafts an RCA + a change-plan draft. |
| `inbound/vcenter-alarm-webhook.yaml` | vCenter / Aria alarm webhook → normalized Incident. |
| `outputs/remediation-bridge.yaml` | **Agent Bridge**: change-plan → vmware-pilot workflow, **HOTL-gated**, audited, auto-rollback. The only path that mutates infra. |
| `outputs/im-notify.yaml` | IM notification (feishu / dingtalk / wecom). |
| `templates/*.j2` | RCA + change-plan markdown. |

## Safety model (inherited from C22 + vmware-skill)

- Advice personas are **read-only** — the runtime denies any tool not tagged
  `[READ]` by the vmware-skill server.
- **No autonomous mutation.** Every change routes through the
  remediation-bridge → vmware-pilot with a **HOTL approval gate before
  confirm**; rejected plans stay un-runnable drafts.
- High-risk changes require explicit `confirmed_by` (never agent-injected).
- Every bridge transition is HMAC-chain audited.

## Install

```bash
xiaoguai pack install packs/vmware-ops/
# enable routes
export XIAOGUAI_VMWARE_OPS_ENABLED=true
```

The vmware-skill family MCP servers must be reachable (launched per the family
standard, `uvx --from <pkg> <pkg>-mcp`) and configured with vCenter/NSX/Aria
connection env (`VSPHERE_*`, `NSX_HOST`, `ARIA_HOST`).

## Deferred

- vmware-monitor scheduled **poll** source (periodic scan → incident).
- `cancel_workflow` for rejected drafts (upstream vmware-pilot gap — VMware-Pilot#7).
- Per-persona evidence-citation enforcement; auto-escalation on high-confidence critical.
