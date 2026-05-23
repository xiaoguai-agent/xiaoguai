//! Assertion evaluation.
//!
//! Every grader is a pure function over (`events`, `messages`) →
//! `Result<(), String>`; the runner aggregates failures into the
//! per-case [`CaseStatus`](crate::types::CaseStatus). Keeping the
//! grader pure makes each variant trivially unit-testable without
//! standing up an agent loop.

use regex::Regex;
use serde_json::Value;
use xiaoguai_agent::AgentEvent;
use xiaoguai_llm::{Message, Role};

use crate::types::{AgentEventPattern, Assertion, ToolCallPattern};

/// Evaluate one assertion against the run record. Returns
/// `Ok(())` on pass, `Err(reason)` on fail. Reasons are
/// human-readable — they go straight into the JSON report.
pub fn check(
    assertion: &Assertion,
    events: &[AgentEvent],
    messages: &[Message],
) -> Result<(), String> {
    match assertion {
        Assertion::FinalMessageContains { text } => check_final_contains(messages, text),
        Assertion::FinalMessageRegex { pattern } => check_final_regex(messages, pattern),
        Assertion::ToolInvocationCount {
            tool_name,
            expected,
        } => check_tool_count(events, tool_name, *expected),
        Assertion::AgentEventSequence { expected } => check_event_sequence(events, expected),
        Assertion::ToolCallSequence { expected } => check_tool_sequence(events, expected),
    }
}

fn final_assistant_text(messages: &[Message]) -> Option<&str> {
    messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, Role::Assistant))
        .map(|m| m.content.as_str())
}

fn check_final_contains(messages: &[Message], needle: &str) -> Result<(), String> {
    let Some(text) = final_assistant_text(messages) else {
        return Err("final_message_contains: no assistant message in transcript".into());
    };
    if text.contains(needle) {
        Ok(())
    } else {
        Err(format!(
            "final_message_contains: expected substring {needle:?} in final assistant text {text:?}"
        ))
    }
}

fn check_final_regex(messages: &[Message], pattern: &str) -> Result<(), String> {
    let re = Regex::new(pattern)
        .map_err(|e| format!("final_message_regex: invalid regex {pattern:?}: {e}"))?;
    let Some(text) = final_assistant_text(messages) else {
        return Err("final_message_regex: no assistant message in transcript".into());
    };
    if re.is_match(text) {
        Ok(())
    } else {
        Err(format!(
            "final_message_regex: pattern {pattern:?} did not match final assistant text {text:?}"
        ))
    }
}

fn check_tool_count(events: &[AgentEvent], tool_name: &str, expected: usize) -> Result<(), String> {
    let actual = events
        .iter()
        .filter(|ev| matches!(ev, AgentEvent::ToolCallStarted { name, .. } if name == tool_name))
        .count();
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "tool_invocation_count: tool {tool_name:?} expected={expected} actual={actual}"
        ))
    }
}

/// Snake-case discriminant for the event type. Mirrors what the
/// `#[serde(tag = "type", rename_all = "snake_case")]` derive
/// produces, but extracted by hand so we don't have to round-trip
/// every event through JSON just to peek at its tag.
fn event_type_tag(ev: &AgentEvent) -> &'static str {
    match ev {
        AgentEvent::TextDelta { .. } => "text_delta",
        AgentEvent::ToolCallStarted { .. } => "tool_call_started",
        AgentEvent::ToolCallFinished { .. } => "tool_call_finished",
        AgentEvent::IterationCompleted { .. } => "iteration_completed",
        AgentEvent::Done { .. } => "done",
        AgentEvent::Error { .. } => "error",
    }
}

fn check_event_sequence(
    events: &[AgentEvent],
    expected: &[AgentEventPattern],
) -> Result<(), String> {
    let mut cursor = 0_usize;
    for ev in events {
        if cursor >= expected.len() {
            return Ok(());
        }
        if event_type_tag(ev) == expected[cursor].event_type {
            cursor += 1;
        }
    }
    if cursor == expected.len() {
        Ok(())
    } else {
        let observed: Vec<&'static str> = events.iter().map(event_type_tag).collect();
        let missing = &expected[cursor..];
        Err(format!(
            "agent_event_sequence: matched {}/{} patterns; missing tail = {:?}; observed = {:?}",
            cursor,
            expected.len(),
            missing
                .iter()
                .map(|p| p.event_type.as_str())
                .collect::<Vec<_>>(),
            observed
        ))
    }
}

fn arguments_match(args_json_raw: &str, substring: &str) -> bool {
    if substring.is_empty() {
        return true;
    }
    if args_json_raw.contains(substring) {
        return true;
    }
    // Re-encode to canonical JSON to absorb whitespace differences
    // between the model's emission and the case's expected payload.
    serde_json::from_str::<Value>(args_json_raw)
        .ok()
        .and_then(|v| serde_json::to_string(&v).ok())
        .is_some_and(|canonical| canonical.contains(substring))
}

fn check_tool_sequence(events: &[AgentEvent], expected: &[ToolCallPattern]) -> Result<(), String> {
    let mut cursor = 0_usize;
    for ev in events {
        if cursor >= expected.len() {
            return Ok(());
        }
        if let AgentEvent::ToolCallStarted {
            name, arguments, ..
        } = ev
        {
            let want = &expected[cursor];
            if name == &want.tool_name {
                let args_str = arguments.to_string();
                if arguments_match(&args_str, &want.arguments_json_substring) {
                    cursor += 1;
                }
            }
        }
    }
    if cursor == expected.len() {
        Ok(())
    } else {
        let missing = &expected[cursor..];
        Err(format!(
            "tool_call_sequence: matched {}/{} patterns; first unmatched = {:?}",
            cursor,
            expected.len(),
            missing.first().map(|p| p.tool_name.as_str())
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assistant(text: &str) -> Message {
        Message::assistant(text)
    }

    fn started(name: &str, args: Value) -> AgentEvent {
        AgentEvent::ToolCallStarted {
            id: "1".into(),
            name: name.into(),
            arguments: args,
        }
    }

    // --- FinalMessageContains -----------------------------------

    #[test]
    fn final_contains_passes_when_substring_present() {
        let msgs = vec![Message::user("hi"), assistant("Hello, world!")];
        check(
            &Assertion::FinalMessageContains {
                text: "Hello".into(),
            },
            &[],
            &msgs,
        )
        .unwrap();
    }

    #[test]
    fn final_contains_fails_with_useful_reason() {
        let msgs = vec![assistant("good morning")];
        let err = check(
            &Assertion::FinalMessageContains {
                text: "evening".into(),
            },
            &[],
            &msgs,
        )
        .unwrap_err();
        assert!(err.contains("evening"), "reason mentions the needle");
        assert!(
            err.contains("good morning"),
            "reason quotes the actual text"
        );
    }

    #[test]
    fn final_contains_fails_when_no_assistant_turn() {
        let msgs = vec![Message::user("hi")];
        let err = check(
            &Assertion::FinalMessageContains { text: "x".into() },
            &[],
            &msgs,
        )
        .unwrap_err();
        assert!(err.contains("no assistant message"));
    }

    // --- FinalMessageRegex --------------------------------------

    #[test]
    fn final_regex_passes() {
        let msgs = vec![assistant("answer: 42")];
        check(
            &Assertion::FinalMessageRegex {
                pattern: r"answer:\s*\d+".into(),
            },
            &[],
            &msgs,
        )
        .unwrap();
    }

    #[test]
    fn final_regex_fails_on_invalid_pattern() {
        let msgs = vec![assistant("anything")];
        let err = check(
            &Assertion::FinalMessageRegex {
                pattern: "(".into(),
            },
            &[],
            &msgs,
        )
        .unwrap_err();
        assert!(err.contains("invalid regex"));
    }

    #[test]
    fn final_regex_fails_when_no_match() {
        let msgs = vec![assistant("hello world")];
        let err = check(
            &Assertion::FinalMessageRegex {
                pattern: r"^\d+$".into(),
            },
            &[],
            &msgs,
        )
        .unwrap_err();
        assert!(err.contains("did not match"));
    }

    // --- ToolInvocationCount ------------------------------------

    #[test]
    fn tool_count_matches_exactly() {
        let events = vec![
            started("search", json!({})),
            started("search", json!({})),
            started("write", json!({})),
        ];
        check(
            &Assertion::ToolInvocationCount {
                tool_name: "search".into(),
                expected: 2,
            },
            &events,
            &[],
        )
        .unwrap();
    }

    #[test]
    fn tool_count_fails_with_actual_in_reason() {
        let events = vec![started("search", json!({}))];
        let err = check(
            &Assertion::ToolInvocationCount {
                tool_name: "search".into(),
                expected: 3,
            },
            &events,
            &[],
        )
        .unwrap_err();
        assert!(err.contains("expected=3"));
        assert!(err.contains("actual=1"));
    }

    // --- AgentEventSequence -------------------------------------

    #[test]
    fn event_sequence_passes_as_subsequence() {
        let events = vec![
            AgentEvent::TextDelta {
                delta: "thinking".into(),
            },
            started("x", json!({})),
            AgentEvent::ToolCallFinished {
                id: "1".into(),
                name: "x".into(),
                ok: true,
                error: None,
                output_text: None,
            },
            AgentEvent::Done {
                stop_reason: xiaoguai_agent::StopReason::Completed,
            },
        ];
        check(
            &Assertion::AgentEventSequence {
                expected: vec![
                    AgentEventPattern {
                        event_type: "tool_call_started".into(),
                    },
                    AgentEventPattern {
                        event_type: "done".into(),
                    },
                ],
            },
            &events,
            &[],
        )
        .unwrap();
    }

    #[test]
    fn event_sequence_fails_when_pattern_missing() {
        let events = vec![AgentEvent::TextDelta { delta: "x".into() }];
        let err = check(
            &Assertion::AgentEventSequence {
                expected: vec![AgentEventPattern {
                    event_type: "done".into(),
                }],
            },
            &events,
            &[],
        )
        .unwrap_err();
        assert!(err.contains("missing tail"));
        assert!(err.contains("done"));
    }

    // --- ToolCallSequence ---------------------------------------

    #[test]
    fn tool_sequence_passes_with_substring_match() {
        let events = vec![
            started("search", json!({"query": "rust"})),
            started("write", json!({"path": "/tmp/a"})),
        ];
        check(
            &Assertion::ToolCallSequence {
                expected: vec![
                    ToolCallPattern {
                        tool_name: "search".into(),
                        arguments_json_substring: "rust".into(),
                    },
                    ToolCallPattern {
                        tool_name: "write".into(),
                        arguments_json_substring: String::new(),
                    },
                ],
            },
            &events,
            &[],
        )
        .unwrap();
    }

    #[test]
    fn tool_sequence_fails_on_argument_substring_mismatch() {
        let events = vec![started("search", json!({"query": "rust"}))];
        let err = check(
            &Assertion::ToolCallSequence {
                expected: vec![ToolCallPattern {
                    tool_name: "search".into(),
                    arguments_json_substring: "python".into(),
                }],
            },
            &events,
            &[],
        )
        .unwrap_err();
        assert!(err.contains("first unmatched"));
        assert!(err.contains("search"));
    }
}
