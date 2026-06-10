//! Cross-transport MCP types.
//!
//! Mirrors a curated subset of rmcp's schema with protocol bookkeeping
//! stripped out. The v0.5.4 agent loop only ever sees these — one boundary
//! to maintain across SDK bumps.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

/// Whether a tool mutates its environment (T5 consult/execute split).
///
/// `Write` is the serde default — unannotated/unknown tools are treated as
/// mutating so consult mode blocks them (fail-closed, plan §2.1). External
/// MCP servers map in via `annotations.readOnlyHint` (see `rmcp_convert`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MutationHint {
    /// The tool only observes — safe in consult (read-only) mode.
    Read,
    /// The tool mutates state (or we don't know — fail-closed default).
    #[default]
    Write,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: Option<String>,
    /// JSON schema for the tool's arguments.
    pub input_schema: JsonValue,
    /// Read/write classification. `#[serde(default)]` keeps descriptors
    /// serialized before this field existed deserializable (→ `Write`).
    #[serde(default)]
    pub mutation_hint: MutationHint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// Flattened text content. MCP supports multi-block content; we
    /// concatenate text blocks here and preserve every block fully in
    /// `blocks` for callers that care about images / resources.
    pub text: String,
    pub blocks: Vec<ContentBlock>,
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        mime_type: String,
        data_base64: String,
    },
    Resource {
        uri: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mutation_hint_defaults_to_write_for_legacy_descriptors() {
        // A descriptor serialized before `mutation_hint` existed must still
        // deserialize — and land on the fail-closed `Write` default.
        let legacy = json!({
            "name": "old_tool",
            "description": "pre-T5 descriptor",
            "input_schema": { "type": "object" }
        });
        let d: ToolDescriptor = serde_json::from_value(legacy).expect("legacy deserializes");
        assert_eq!(d.mutation_hint, MutationHint::Write);
    }

    #[test]
    fn explicit_read_hint_round_trips() {
        let d = ToolDescriptor {
            name: "lookup".into(),
            description: None,
            input_schema: json!({ "type": "object" }),
            mutation_hint: MutationHint::Read,
        };
        let wire = serde_json::to_value(&d).expect("serializes");
        assert_eq!(wire["mutation_hint"], "read");
        let back: ToolDescriptor = serde_json::from_value(wire).expect("deserializes");
        assert_eq!(back.mutation_hint, MutationHint::Read);
    }

    #[test]
    fn default_trait_is_write() {
        assert_eq!(MutationHint::default(), MutationHint::Write);
    }
}
