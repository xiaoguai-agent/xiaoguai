//! Encode `AgentEvent`s as Server-Sent Events.
//!
//! The wire format is one SSE event per `AgentEvent`. Each event's `event:`
//! field carries the variant tag (`text_delta`, `tool_call_started`, ...) so
//! browsers can subscribe selectively, and the `data:` payload is the
//! variant's serde-JSON body.

use axum::response::sse::Event;
use xiaoguai_agent::AgentEvent;

/// Convert one `AgentEvent` into a typed SSE `Event`. Errors are serialised
/// as a regular `error` event so the stream stays well-formed even on
/// adverse paths.
pub fn event_to_sse(ev: &AgentEvent) -> Event {
    let (name, body) = match ev {
        AgentEvent::TextDelta { .. } => ("text_delta", serde_json::to_value(ev)),
        AgentEvent::ToolCallStarted { .. } => ("tool_call_started", serde_json::to_value(ev)),
        AgentEvent::ToolCallFinished { .. } => ("tool_call_finished", serde_json::to_value(ev)),
        AgentEvent::IterationCompleted { .. } => ("iteration_completed", serde_json::to_value(ev)),
        AgentEvent::Done { .. } => ("done", serde_json::to_value(ev)),
        AgentEvent::Error { .. } => ("error", serde_json::to_value(ev)),
        AgentEvent::HotlPending { .. } => ("hotl_pending", serde_json::to_value(ev)),
        AgentEvent::HotlResolved { .. } => ("hotl_resolved", serde_json::to_value(ev)),
    };
    let json = body.unwrap_or_else(
        |e| serde_json::json!({"type": "error", "message": format!("encode: {e}")}),
    );
    Event::default().event(name).data(json.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;
    use xiaoguai_agent::HotlResolution;

    #[test]
    fn text_delta_event_carries_text_delta_tag() {
        let ev = AgentEvent::TextDelta { delta: "hi".into() };
        let sse = event_to_sse(&ev);
        // Inspect via the raw SSE serialisation — axum's Event Display impl
        // emits `event: <name>\ndata: <body>\n\n`.
        let rendered = format!("{sse:?}");
        assert!(rendered.contains("text_delta"));
    }

    #[test]
    fn hotl_pending_encodes_as_sse_event() {
        let request_id = Uuid::new_v4();
        let ev = AgentEvent::HotlPending {
            request_id,
            tool: "execute_python".into(),
            args_redacted: serde_json::json!({"code": "[redacted]"}),
            scope: "tool_call.execute_python".into(),
            expires_at: Utc.with_ymd_and_hms(2026, 5, 31, 8, 12, 34).unwrap(),
        };
        let sse = event_to_sse(&ev);
        let rendered = format!("{sse:?}");
        assert!(
            rendered.contains("hotl_pending"),
            "expected event name `hotl_pending` in SSE: {rendered}"
        );
        assert!(
            rendered.contains("execute_python"),
            "expected serialised data to include tool name: {rendered}"
        );
        assert!(
            rendered.contains(&request_id.to_string()),
            "expected serialised data to include request_id: {rendered}"
        );
    }

    #[test]
    fn hotl_resolved_encodes_as_sse_event() {
        let request_id = Uuid::new_v4();
        let ev = AgentEvent::HotlResolved {
            request_id,
            verdict: HotlResolution::Allow,
            decided_by: Some("ops@acme.com".into()),
            recorded_at: Utc.with_ymd_and_hms(2026, 5, 30, 8, 13, 1).unwrap(),
        };
        let sse = event_to_sse(&ev);
        let rendered = format!("{sse:?}");
        assert!(
            rendered.contains("hotl_resolved"),
            "expected event name `hotl_resolved` in SSE: {rendered}"
        );
        // Verdict must be lowercased on the wire (api-contract §2.6.3).
        assert!(
            rendered.contains("\\\"verdict\\\":\\\"allow\\\"")
                || rendered.contains("\"verdict\":\"allow\""),
            "expected lowercase `\"verdict\":\"allow\"` in SSE: {rendered}"
        );
    }
}
