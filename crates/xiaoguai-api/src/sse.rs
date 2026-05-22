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
    };
    let json = body.unwrap_or_else(
        |e| serde_json::json!({"type": "error", "message": format!("encode: {e}")}),
    );
    Event::default().event(name).data(json.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_delta_event_carries_text_delta_tag() {
        let ev = AgentEvent::TextDelta { delta: "hi".into() };
        let sse = event_to_sse(&ev);
        // Inspect via the raw SSE serialisation — axum's Event Display impl
        // emits `event: <name>\ndata: <body>\n\n`.
        let rendered = format!("{sse:?}");
        assert!(rendered.contains("text_delta"));
    }
}
