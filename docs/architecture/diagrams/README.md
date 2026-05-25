# Architecture Diagrams — Wave 3

Mermaid diagrams covering the subsystems introduced in wave 3
(v1.2.x / v1.3.x-prep). Each file contains a short prose context,
the Mermaid code block, and a Related section pointing to the
relevant ADR and source crate.

| Diagram | Kind | What it shows |
|---------|------|---------------|
| [hotl-request-flow.md](hotl-request-flow.md) | Sequence | Full HotL-gated request: API → enforcer → policy store → usage log → Allow / Escalate / Deny → IM fanout → operator ack |
| [outcome-attribution-chain.md](outcome-attribution-chain.md) | Graph | Single-hop, multi-hop, and branching attribution chains; how readers walk `parent_outcome_id` |
| [pack-install-lifecycle.md](pack-install-lifecycle.md) | State | Skill pack states from catalog through install → DB row → (v1.3) loader activation → live → uninstall → archived; v1.2 no-op caveat annotated |
| [rate-limit-decision-path.md](rate-limit-decision-path.md) | Flowchart | Request → auth → rate-class lookup → in-mem / Valkey backend → allow / 429; relationship to HotL layer |
| [wave3-system-overview.md](wave3-system-overview.md) | C4 Component | All wave-3 subsystems and their connections to core runtime, scheduler, audit, IM adapters, RAG, and cloud LLMs |

## Rendering

Diagrams render in:

- GitHub (native Mermaid support in `.md` files)
- VS Code with the [Mermaid Preview](https://marketplace.visualstudio.com/items?itemName=bierner.markdown-mermaid) extension
- Any Mermaid-enabled viewer at [mermaid.live](https://mermaid.live)

## Related docs

- Design: `docs/architecture/2026-05-21-design.md`
- ADR index: `docs/architecture/adr/`
- Wave-3 handoff: `docs/HANDOFF-2026-05-26.md`
