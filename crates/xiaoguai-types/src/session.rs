//! Session + message domain types.

use crate::ids::{MessageId, SessionId, UserId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub user_id: UserId,
    pub title: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub model: String,
    pub status: SessionStatus,
    /// v1.1.2 — when this session was created via "branch from here",
    /// the ID of the session it forked off of. `None` for top-level
    /// sessions (the overwhelming majority).
    #[serde(default)]
    pub parent_session_id: Option<SessionId>,
    /// v1.1.2 — companion to `parent_session_id`: the last message
    /// from the parent that was copied into this session at fork time.
    #[serde(default)]
    pub forked_from_message_id: Option<MessageId>,
    /// Feature ⑤ — per-session coding workspace root: an absolute server
    /// path used as the coding tools' workspace base for this session's
    /// turns. `None`/empty falls back to the global default
    /// (`XIAOGUAI_CODING_WORKSPACE`). Settable via `PATCH /v1/sessions/{id}`.
    #[serde(default)]
    pub working_dir: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Active,
    Archived,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub session_id: SessionId,
    pub role: Role,
    pub content: Vec<ContentBlock>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolCall {
        tool_call_id: String,
        name: String,
        arguments: serde_json::Value,
    },
    ToolResult {
        tool_call_id: String,
        output: serde_json::Value,
        is_error: bool,
    },
    /// v0.9.3 — RAG citation. Renders as a click-to-source chip
    /// alongside the assistant turn that referenced it. The schema
    /// mirrors `xiaoguai_rag::Citation` so the two layers can
    /// round-trip JSON without a bespoke converter.
    ///
    /// Hard rule: every RAG-emitted assistant turn that uses retrieved
    /// content MUST include at least one `Citation` block per source.
    /// Unsourced retrieved text is what every competing product
    /// (`OpenWebUI` / `AnythingLLM`) gets wrong; the type system
    /// enforces the contract here.
    Citation {
        /// `file://`, `https://`, or a custom scheme (e.g. `obsidian://`,
        /// `r2r://doc/<id>`).
        source_uri: String,
        /// Inclusive 1-indexed `[start, end]` line numbers. `(0, 0)`
        /// when the backend can't produce lines — the chat-ui treats
        /// that as "no anchor" and falls back to a whole-document link.
        span: (u32, u32),
        /// Retrieval score in `[0, 1]`. UI uses it for sort order +
        /// chip opacity.
        score: f32,
        /// Chunk preview shown in the hover card (~200-400 chars).
        preview: String,
        /// Collection that produced the hit, for "find more from this
        /// source" actions.
        collection_id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn citation_block_serde_round_trip() {
        let block = ContentBlock::Citation {
            source_uri: "file:///x/notes.md".into(),
            span: (3, 7),
            score: 0.84,
            preview: "found the needle in the haystack".into(),
            collection_id: "notes".into(),
        };
        let json = serde_json::to_value(&block).expect("ser");
        assert_eq!(json["type"], "citation");
        assert_eq!(json["source_uri"], "file:///x/notes.md");
        assert_eq!(json["span"], json!([3, 7]));
        assert_eq!(json["collection_id"], "notes");

        let back: ContentBlock = serde_json::from_value(json).expect("de");
        match back {
            ContentBlock::Citation {
                source_uri,
                span,
                score,
                preview,
                collection_id,
            } => {
                assert_eq!(source_uri, "file:///x/notes.md");
                assert_eq!(span, (3, 7));
                assert!((score - 0.84).abs() < 1e-5);
                assert!(preview.contains("needle"));
                assert_eq!(collection_id, "notes");
            }
            _ => panic!("expected citation"),
        }
    }

    #[test]
    fn unknown_block_type_fails_deserialize() {
        let v = json!({ "type": "bogus", "text": "x" });
        let r: Result<ContentBlock, _> = serde_json::from_value(v);
        assert!(r.is_err());
    }

    #[test]
    fn citation_block_with_no_anchor_uses_zero_span() {
        let block = ContentBlock::Citation {
            source_uri: "https://example.com/doc".into(),
            span: (0, 0),
            score: 0.5,
            preview: "abc".into(),
            collection_id: "c".into(),
        };
        let s = serde_json::to_string(&block).unwrap();
        assert!(
            s.contains("\"span\":[0,0]"),
            "span should round-trip as [0,0]: {s}"
        );
    }
}
