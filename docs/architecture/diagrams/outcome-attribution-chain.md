# Outcome Attribution Chain

Outcome telemetry lets any agent session claim credit for a measurable
business result — revenue, cost savings, hours reclaimed, etc. Each
`OutcomeRecord` carries an optional `parent_outcome_id` that forms a
directed acyclic graph. A reader walks the chain from a leaf node back
to the root input event to reconstruct the full attribution path. The
diagram covers three shapes: single-hop (agent → outcome), multi-hop
(agent A → agent B → outcome), and branching (one agent spawning
multiple tool calls, each with its own attributed outcome).

```mermaid
graph TD
    IE["Input Event<br/>(session_id: S1)"]

    subgraph single ["Single-hop attribution"]
        A1["Agent A<br/>session S1"]
        O1["OutcomeRecord<br/>kind=revenue, value=$120<br/>parent_outcome_id=null"]
        A1 --> O1
    end

    subgraph multihop ["Multi-hop attribution"]
        A2["Agent A<br/>session S2"]
        A3["Agent B<br/>session S3, parent=S2"]
        O2["OutcomeRecord<br/>kind=cost_saving, value=2h<br/>parent_outcome_id=O_A2"]
        A2 --> A3
        A3 --> O2
        O_A2["OutcomeRecord (A2 step)<br/>kind=hours_saved, value=0.5h<br/>parent_outcome_id=null"]
        A2 --> O_A2
    end

    subgraph branching ["Branching attribution"]
        A4["Agent C<br/>session S4"]
        TC1["Tool Call: search_crm"]
        TC2["Tool Call: draft_email"]
        TC3["Tool Call: book_meeting"]
        O3["OutcomeRecord<br/>kind=revenue, value=$300<br/>parent=TC1_outcome"]
        O4["OutcomeRecord<br/>kind=hours_saved, value=1h<br/>parent=TC2_outcome"]
        O5["OutcomeRecord<br/>kind=custom (meeting booked)<br/>parent=TC3_outcome"]
        A4 --> TC1
        A4 --> TC2
        A4 --> TC3
        TC1 --> O3
        TC2 --> O4
        TC3 --> O5
    end

    IE --> A1
    IE --> A2
    IE --> A4

    Reader["Admin UI / ROI Report<br/>walks parent_outcome_id chain"]
    O1 --> Reader
    O2 --> Reader
    O3 & O4 & O5 --> Reader
```

## Related

- **ADR**: `docs/architecture/adr/0008-tool-result-provenance.md`
- **Source crates**:
  - Outcome types + in-memory recorder: `crates/xiaoguai-audit/src/outcomes.rs`
  - REST API surface: `crates/xiaoguai-api/src/outcomes.rs`
  - Routes: `crates/xiaoguai-api/src/routes/outcomes.rs`
  - PG bridge (v1.3): `crates/xiaoguai-core/src/outcomes_bridge.rs` (planned)
- **Migration**: `migrations/0012_outcomes.sql`
