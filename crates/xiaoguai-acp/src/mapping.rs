//! Pure translation from `xiaoguai-agent` loop events to ACP wire updates.
//!
//! Kept side-effect-free and unit-tested on the JSON shape so the
//! `sessionUpdate` discriminator and camelCase keys (both the schema crate's
//! responsibility) are pinned by tests, per `LLD-ACP-001` §4/§8.

use crate::acp::{
    ContentBlock, ContentChunk, SessionUpdate, StopReason, ToolCall, ToolCallContent,
    ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use xiaoguai_agent::{AgentEvent, StopReason as AgentStop};

/// Map one loop event to at most one ACP `SessionUpdate`.
///
/// Returns `None` for events with no ACP analogue (`IterationCompleted`) and
/// for the terminal `Done` (whose stop reason becomes the `PromptResponse`,
/// not a notification). `HotlPending`/`HotlResolved` are `None` in P2 — the
/// interactive `session/request_permission` flow is deferred (`LLD-ACP-001`
/// §6); the CLI runs allow-all so turns never suspend.
#[must_use]
pub fn map_event(ev: &AgentEvent) -> Option<SessionUpdate> {
    match ev {
        AgentEvent::TextDelta { delta } => Some(SessionUpdate::AgentMessageChunk(
            ContentChunk::new(ContentBlock::from(delta.clone())),
        )),
        AgentEvent::ToolCallStarted { id, name, .. } => Some(SessionUpdate::ToolCall(
            ToolCall::new(id.clone(), name.clone())
                .kind(tool_kind(name))
                .status(ToolCallStatus::Pending),
        )),
        AgentEvent::ToolCallFinished {
            id,
            ok,
            output_text,
            error,
            ..
        } => {
            let status = if *ok {
                ToolCallStatus::Completed
            } else {
                ToolCallStatus::Failed
            };
            let text = error.clone().or_else(|| output_text.clone());
            let mut fields = ToolCallUpdateFields::new().status(status);
            if let Some(text) = text {
                let block: ToolCallContent = ContentBlock::from(text).into();
                fields = fields.content(vec![block]);
            }
            Some(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                id.clone(),
                fields,
            )))
        }
        AgentEvent::Error { message } => Some(SessionUpdate::AgentMessageChunk(ContentChunk::new(
            ContentBlock::from(format!("⚠ {message}")),
        ))),
        AgentEvent::IterationCompleted { .. }
        | AgentEvent::Done { .. }
        | AgentEvent::HotlPending { .. }
        | AgentEvent::HotlResolved { .. } => None,
    }
}

/// Map the loop's terminal stop reason to the ACP `StopReason`.
#[must_use]
pub fn map_stop_reason(stop: &AgentStop) -> StopReason {
    match stop {
        AgentStop::Completed => StopReason::EndTurn,
        AgentStop::MaxIterations => StopReason::MaxTurnRequests,
        AgentStop::Cancelled => StopReason::Cancelled,
    }
}

/// Best-effort tool name → ACP `ToolKind` (display hint only; editors use it to
/// pick an icon). Matches the `xiaoguai-coding` tool vocabulary (`LLD-CODING-001`).
fn tool_kind(name: &str) -> ToolKind {
    match name {
        "edit_file" => ToolKind::Edit,
        "read_file" | "list_dir" | "grep" | "git_status" | "git_diff" => ToolKind::Read,
        "run_command" | "git_add" | "git_commit" | "git_branch" | "git_push" | "execute_python" => {
            ToolKind::Execute
        }
        // `open_pr` and everything unrecognised fall through to `Other`.
        _ => ToolKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn to_json(u: &SessionUpdate) -> serde_json::Value {
        serde_json::to_value(u).expect("serialize")
    }

    #[test]
    fn text_delta_becomes_agent_message_chunk() {
        let u = map_event(&AgentEvent::TextDelta {
            delta: "hello".into(),
        })
        .unwrap();
        let v = to_json(&u);
        assert_eq!(v["sessionUpdate"], "agent_message_chunk");
        assert_eq!(v["content"]["type"], "text");
        assert_eq!(v["content"]["text"], "hello");
    }

    #[test]
    fn tool_call_started_is_pending_with_kind() {
        let u = map_event(&AgentEvent::ToolCallStarted {
            id: "tc-1".into(),
            name: "edit_file".into(),
            arguments: json!({"path": "a.rs"}),
        })
        .unwrap();
        let v = to_json(&u);
        assert_eq!(v["sessionUpdate"], "tool_call");
        assert_eq!(v["toolCallId"], "tc-1");
        assert_eq!(v["title"], "edit_file");
        assert_eq!(v["kind"], "edit");
        // `pending` is the schema default and is omitted from the wire (the
        // client infers it); a non-default status would serialize.
        assert!(
            v.get("status").is_none(),
            "pending status should be omitted"
        );
    }

    #[test]
    fn tool_call_finished_ok_completes_with_output() {
        let u = map_event(&AgentEvent::ToolCallFinished {
            id: "tc-1".into(),
            name: "grep".into(),
            ok: true,
            error: None,
            output_text: Some("3 matches".into()),
        })
        .unwrap();
        let v = to_json(&u);
        assert_eq!(v["sessionUpdate"], "tool_call_update");
        assert_eq!(v["toolCallId"], "tc-1");
        assert_eq!(v["status"], "completed");
        assert_eq!(v["content"][0]["content"]["text"], "3 matches");
    }

    #[test]
    fn tool_call_finished_err_fails_with_error_text() {
        let u = map_event(&AgentEvent::ToolCallFinished {
            id: "tc-2".into(),
            name: "run_command".into(),
            ok: false,
            error: Some("exit 1".into()),
            output_text: None,
        })
        .unwrap();
        let v = to_json(&u);
        assert_eq!(v["status"], "failed");
        assert_eq!(v["content"][0]["content"]["text"], "exit 1");
    }

    #[test]
    fn error_event_surfaces_as_chunk() {
        let u = map_event(&AgentEvent::Error {
            message: "boom".into(),
        })
        .unwrap();
        let v = to_json(&u);
        assert_eq!(v["sessionUpdate"], "agent_message_chunk");
        assert_eq!(v["content"]["text"], "⚠ boom");
    }

    #[test]
    fn non_mapped_events_are_none() {
        assert!(map_event(&AgentEvent::IterationCompleted { iteration: 1 }).is_none());
        assert!(map_event(&AgentEvent::Done {
            stop_reason: AgentStop::Completed
        })
        .is_none());
    }

    #[test]
    fn stop_reasons_map() {
        assert!(matches!(
            map_stop_reason(&AgentStop::Completed),
            StopReason::EndTurn
        ));
        assert!(matches!(
            map_stop_reason(&AgentStop::MaxIterations),
            StopReason::MaxTurnRequests
        ));
        assert!(matches!(
            map_stop_reason(&AgentStop::Cancelled),
            StopReason::Cancelled
        ));
    }
}
