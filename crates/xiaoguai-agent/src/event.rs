//! Agent-loop events surfaced to callers.
//!
//! The ReAct loop streams these as it executes so an upstream `xiaoguai-api`
//! handler can forward them over SSE/WebSocket without coupling to the loop's
//! internals.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// Streamed model text. Multiple events compose to the full assistant turn.
    TextDelta { delta: String },

    /// Model decided to call a tool. Emitted per call, before dispatch.
    ToolCallStarted {
        id: String,
        name: String,
        arguments: JsonValue,
    },

    /// Tool dispatch completed (success or failure).
    ToolCallFinished {
        id: String,
        name: String,
        ok: bool,
        /// MCP-side error message when `ok == false`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// MCP-side text payload (concatenated from text blocks).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_text: Option<String>,
    },

    /// One think→act→observe cycle completed.
    IterationCompleted { iteration: u32 },

    /// Loop terminated. No further events follow.
    Done { stop_reason: StopReason },

    /// Unrecoverable error mid-loop. No further events follow.
    Error { message: String },

    /// New (sprint-12). Tool dispatch paused; waiting on operator decision.
    /// SSE event name: `hotl_pending`. Wire shape: `api-contract.md` §2.6.3.
    HotlPending {
        request_id: Uuid,
        tool: String,
        args_redacted: JsonValue,
        scope: String,
        expires_at: DateTime<Utc>,
    },

    /// New (sprint-12). Emitted after the ticket resolves.
    /// SSE event name: `hotl_resolved`. Wire shape: `api-contract.md` §2.6.3.
    /// `decided_by` is omitted on `Timeout`.
    HotlResolved {
        request_id: Uuid,
        verdict: HotlResolution,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decided_by: Option<String>,
        recorded_at: DateTime<Utc>,
    },
}

/// Operator decision verdict for a suspended tool-call.
///
/// TODO(sprint-12 S12-1 merge): unify with `crate::hotl_gate::HotlResolution`
/// once S12-1 lands its parallel-sub-agent enum. Both must serialise to the
/// same lowercase wire shape (`allow` / `deny` / `timeout`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HotlResolution {
    Allow,
    Deny,
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Model emitted a `finish_reason = stop` without further tool calls.
    Completed,
    /// Hit `AgentConfig::max_iterations` before the model stopped.
    MaxIterations,
    /// Caller signalled the cancellation token.
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn hotl_pending_round_trip() {
        let request_id = Uuid::new_v4();
        let expires_at = Utc.with_ymd_and_hms(2026, 5, 31, 8, 12, 34).unwrap();
        let ev = AgentEvent::HotlPending {
            request_id,
            tool: "execute_python".into(),
            args_redacted: serde_json::json!({"code": "[redacted]"}),
            scope: "tool_call.execute_python".into(),
            expires_at,
        };

        let json = serde_json::to_value(&ev).expect("serialize");
        assert_eq!(json["type"], "hotl_pending");
        assert_eq!(json["tool"], "execute_python");
        assert_eq!(json["scope"], "tool_call.execute_python");
        assert_eq!(json["request_id"], request_id.to_string());
        assert_eq!(json["expires_at"], "2026-05-31T08:12:34Z");

        let back: AgentEvent = serde_json::from_value(json).expect("deserialize");
        match back {
            AgentEvent::HotlPending {
                request_id: rid,
                tool,
                scope,
                expires_at: ea,
                ..
            } => {
                assert_eq!(rid, request_id);
                assert_eq!(tool, "execute_python");
                assert_eq!(scope, "tool_call.execute_python");
                assert_eq!(ea, expires_at);
            }
            other => panic!("expected HotlPending, got {other:?}"),
        }
    }

    #[test]
    fn hotl_resolved_allow_round_trip() {
        let request_id = Uuid::new_v4();
        let recorded_at = Utc.with_ymd_and_hms(2026, 5, 30, 8, 13, 1).unwrap();
        let ev = AgentEvent::HotlResolved {
            request_id,
            verdict: HotlResolution::Allow,
            decided_by: Some("ops@acme.com".into()),
            recorded_at,
        };

        let json = serde_json::to_value(&ev).expect("serialize");
        assert_eq!(json["type"], "hotl_resolved");
        assert_eq!(json["verdict"], "allow");
        assert_eq!(json["decided_by"], "ops@acme.com");
        assert_eq!(json["recorded_at"], "2026-05-30T08:13:01Z");

        let back: AgentEvent = serde_json::from_value(json).expect("deserialize");
        match back {
            AgentEvent::HotlResolved {
                request_id: rid,
                verdict,
                decided_by,
                recorded_at: ra,
            } => {
                assert_eq!(rid, request_id);
                assert!(matches!(verdict, HotlResolution::Allow));
                assert_eq!(decided_by.as_deref(), Some("ops@acme.com"));
                assert_eq!(ra, recorded_at);
            }
            other => panic!("expected HotlResolved, got {other:?}"),
        }
    }

    #[test]
    fn hotl_resolved_timeout_serialises_lowercase() {
        // Per api-contract §2.6.3: `decided_by` is omitted when verdict = "timeout".
        let ev = AgentEvent::HotlResolved {
            request_id: Uuid::new_v4(),
            verdict: HotlResolution::Timeout,
            decided_by: None,
            recorded_at: Utc.with_ymd_and_hms(2026, 5, 30, 8, 13, 1).unwrap(),
        };

        let json = serde_json::to_value(&ev).expect("serialize");
        assert_eq!(json["verdict"], "timeout");
        assert!(
            json.get("decided_by").map_or(true, |v| v.is_null()) || !json.as_object().unwrap().contains_key("decided_by"),
            "decided_by must be omitted on timeout, got: {json}"
        );

        // And `deny` also lower-cases.
        let deny = AgentEvent::HotlResolved {
            request_id: Uuid::new_v4(),
            verdict: HotlResolution::Deny,
            decided_by: Some("ops@acme.com".into()),
            recorded_at: Utc.with_ymd_and_hms(2026, 5, 30, 8, 13, 1).unwrap(),
        };
        let json = serde_json::to_value(&deny).expect("serialize");
        assert_eq!(json["verdict"], "deny");
    }
}
