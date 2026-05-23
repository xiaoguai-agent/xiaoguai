//! Sketch: how a regression `EvalCase` is built from a
//! production `sessions.id` + its `audit_log` rows.
//!
//! Roadmap §5.4 says the eval substrate IS the existing
//! `sessions` + `audit_log` tables — no new DB. v0.11.0 ships the
//! shape; v0.11.2 (the eval pane) wires a one-click "convert prod
//! run to regression case" button against the same flow.
//!
//! The canonical query pattern is:
//!
//! ```sql
//! -- Input messages: the user-role rows up to the first assistant
//! -- turn we want to regression-test.
//! SELECT role, content
//! FROM messages
//! WHERE session_id = $1
//! ORDER BY created_at ASC;
//!
//! -- Tool-call sequence: extracted from `audit_log.details` for
//! -- actor LIKE 'agent:%' on the same session.
//! SELECT details->>'tool_name'    AS tool_name,
//!        details->'arguments'     AS arguments
//! FROM audit_log
//! WHERE action = 'tool.invoke'
//!   AND details->>'session_id' = $1
//! ORDER BY ts ASC;
//! ```
//!
//! The mock script is then built turn-by-turn: each assistant
//! turn that emitted tool calls becomes a [`MockTurn::tool_calls`]
//! step; the final assistant turn becomes a [`MockTurn::text`]
//! step. Assertions are derived from operator intent ("this final
//! message must mention X", "this tool must be called exactly once").
//!
//! This test exercises the *translation function* with a hand-
//! built audit-log shape so any change to the canonical schema
//! breaks the test loudly. The real PG query lives in v0.11.2.

use serde_json::json;
use xiaoguai_audit::AuditEntry;
use xiaoguai_eval::{Assertion, EvalCase, MockScript, MockTurn, ToolCallPattern};
use xiaoguai_llm::{Message, ToolCallSpec};

/// Translate a (`input_messages`, `audit_log` slice, assertions)
/// triple into an [`EvalCase`]. The audit slice carries one
/// `tool.invoke` row per real tool call; this function projects
/// those into a [`MockScript`] that, when fed to `MockBackend`,
/// replays the same tool-call sequence.
///
/// Production wiring will live in `xiaoguai-storage` (where the
/// PG types are in scope); the function lives here as a hand-
/// built test fixture so the shape is documented.
fn case_from_audit(
    id: impl Into<String>,
    input_messages: Vec<Message>,
    audit_rows: &[AuditEntry],
    final_text: impl Into<String>,
    assertions: Vec<Assertion>,
) -> EvalCase {
    let mut turns: Vec<MockTurn> = Vec::new();
    for row in audit_rows {
        if row.action != "tool.invoke" {
            continue;
        }
        let Some(tool_name) = row.details.get("tool_name").and_then(|v| v.as_str()) else {
            continue;
        };
        let args = row
            .details
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let arguments_json = serde_json::to_string(&args).unwrap_or_else(|_| "{}".into());
        turns.push(MockTurn::tool_calls(vec![ToolCallSpec {
            id: format!("call-{}", turns.len() + 1),
            name: tool_name.into(),
            arguments_json,
        }]));
    }
    // Final turn: the assistant text the model produced after the
    // last tool result. Without it the agent would loop forever.
    turns.push(MockTurn::text(final_text.into()));

    EvalCase {
        id: id.into(),
        input_messages,
        mock_script: Some(MockScript::new(turns)),
        assertions,
        tags: vec!["regression".into(), "from-audit".into()],
    }
}

#[test]
fn case_from_audit_recovers_tool_sequence() {
    let input = vec![Message::user("look up the weather and tell me")];
    let audit_rows = vec![
        AuditEntry {
            ts: chrono::Utc::now(),
            tenant_id: "alice".into(),
            actor: "agent:react".into(),
            action: "tool.invoke".into(),
            resource: Some("session:S1".into()),
            details: json!({
                "session_id": "S1",
                "tool_name": "weather_lookup",
                "arguments": {"city": "Berlin"},
            }),
        },
        // A non-tool audit row should be ignored by the projection.
        AuditEntry {
            ts: chrono::Utc::now(),
            tenant_id: "alice".into(),
            actor: "agent:react".into(),
            action: "cost.charge".into(),
            resource: Some("session:S1".into()),
            details: json!({"session_id": "S1", "tokens_in": 12}),
        },
    ];

    let case = case_from_audit(
        "weather-regression-1",
        input,
        &audit_rows,
        "It's 18°C and sunny in Berlin.",
        vec![
            Assertion::ToolCallSequence {
                expected: vec![ToolCallPattern {
                    tool_name: "weather_lookup".into(),
                    arguments_json_substring: "Berlin".into(),
                }],
            },
            Assertion::FinalMessageContains {
                text: "Berlin".into(),
            },
        ],
    );

    let script = case.mock_script.as_ref().expect("script populated");
    assert_eq!(
        script.turns.len(),
        2,
        "one tool-call turn + one final-text turn"
    );
    assert_eq!(script.turns[0].tool_calls.len(), 1);
    assert_eq!(script.turns[0].tool_calls[0].name, "weather_lookup");
    assert!(script.turns[0].tool_calls[0]
        .arguments_json
        .contains("Berlin"));
    assert_eq!(script.turns[1].text, "It's 18°C and sunny in Berlin.");
    assert_eq!(case.tags, vec!["regression", "from-audit"]);
}
