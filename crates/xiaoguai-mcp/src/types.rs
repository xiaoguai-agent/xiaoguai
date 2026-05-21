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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: Option<String>,
    /// JSON schema for the tool's arguments.
    pub input_schema: JsonValue,
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
