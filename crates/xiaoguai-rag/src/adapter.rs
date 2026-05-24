//! `RagMcpAdapter` — wraps any `RagClient` and implements
//! `xiaoguai_mcp::McpClient` so RAG slots into the existing `Toolbox`
//! exactly like any other MCP server.
//!
//! Four tools exposed:
//!
//! * `rag_search` — `{collection_id, query, top_k?, min_score?}` →
//!   `{hits: [{document_id, source_uri, span:[s,e], score, preview}], elapsed_ms}`
//! * `rag_ingest` — `{collection_id, source_uri, content, metadata?}` →
//!   `{document_id, chunk_count}`
//! * `rag_list_collections` — `{}` → `{collections: [...]}`
//! * `rag_delete_document` — `{collection_id, document_id}` → `{deleted: true}`
//!
//! Underscore (not dot) in tool names because the OpenAI tool-call
//! spec is restrictive about characters; OpenAI returns 400 on names
//! containing `.` — confirmed via Anthropic / OpenAI compatibility
//! testing. Roadmap principle: "agent UX over branding".

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use xiaoguai_mcp::{
    ContentBlock, McpClient, McpError, McpResult, ServerInfo, ToolDescriptor, ToolResult,
};

use crate::client::{RagClient, RagError};
use crate::types::{IngestRequest, SearchRequest};

pub struct RagMcpAdapter {
    inner: Arc<dyn RagClient>,
    server_name: String,
}

impl std::fmt::Debug for RagMcpAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RagMcpAdapter")
            .field("server_name", &self.server_name)
            .finish_non_exhaustive()
    }
}

impl RagMcpAdapter {
    #[must_use]
    pub fn new(inner: Arc<dyn RagClient>) -> Self {
        Self {
            inner,
            server_name: "xiaoguai-rag".into(),
        }
    }
}

fn map_rag_err(e: RagError) -> McpError {
    match e {
        RagError::NotFound(s) => McpError::InvalidArgument(format!("not found: {s}")),
        RagError::InvalidArgument(s) => McpError::InvalidArgument(s),
        RagError::Backend(s) => McpError::Transport(s),
        // v0.12.2: backends that don't implement an optional method
        // (today only `reindex_path`) surface as Unsupported. Map to
        // InvalidArgument so the MCP tool surface stays predictable
        // (agents see a "this isn't supported" message, not a transport
        // error that would hint at a wire-level problem).
        RagError::Unsupported(s) => McpError::InvalidArgument(format!("unsupported: {s}")),
    }
}

#[async_trait]
impl McpClient for RagMcpAdapter {
    async fn initialize(&self) -> McpResult<ServerInfo> {
        Ok(ServerInfo {
            name: self.server_name.clone(),
            version: env!("CARGO_PKG_VERSION").into(),
        })
    }

    async fn list_tools(&self) -> McpResult<Vec<ToolDescriptor>> {
        Ok(vec![
            ToolDescriptor {
                name: "rag_search".into(),
                description: Some(
                    "Search a RAG collection for chunks matching a query. \
                     Returns citation-anchored hits (source_uri + line span + score)."
                        .into(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "collection_id": { "type": "string" },
                        "query":         { "type": "string" },
                        "top_k":         { "type": "integer", "minimum": 1, "default": 8 },
                        "min_score":     { "type": "number",  "minimum": 0, "maximum": 1 }
                    },
                    "required": ["collection_id", "query"]
                }),
            },
            ToolDescriptor {
                name: "rag_ingest".into(),
                description: Some(
                    "Ingest a single document into a collection. Idempotent on source_uri.".into(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "collection_id": { "type": "string" },
                        "source_uri":    { "type": "string" },
                        "content":       { "type": "string" },
                        "metadata":      { "type": "object" }
                    },
                    "required": ["collection_id", "source_uri", "content"]
                }),
            },
            ToolDescriptor {
                name: "rag_list_collections".into(),
                description: Some("List every collection visible to the caller.".into()),
                input_schema: json!({ "type": "object", "properties": {} }),
            },
            ToolDescriptor {
                name: "rag_delete_document".into(),
                description: Some("Remove a document by id. Idempotent — missing is OK.".into()),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "collection_id": { "type": "string" },
                        "document_id":   { "type": "string" }
                    },
                    "required": ["collection_id", "document_id"]
                }),
            },
        ])
    }

    async fn call_tool(&self, name: &str, args: JsonValue) -> McpResult<ToolResult> {
        let obj = match args {
            JsonValue::Object(m) => m,
            JsonValue::Null => serde_json::Map::new(),
            other => {
                return Err(McpError::InvalidArgument(format!(
                    "arguments must be an object or null, got: {other}"
                )));
            }
        };

        let val = |key: &str| obj.get(key).cloned().unwrap_or(JsonValue::Null);
        let str_arg = |key: &str| {
            val(key)
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| McpError::InvalidArgument(format!("{key} must be a string")))
        };

        let result_json: JsonValue = match name {
            "rag_search" => {
                let collection_id = str_arg("collection_id")?;
                let query = str_arg("query")?;
                let top_k = val("top_k")
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .unwrap_or(8);
                #[allow(clippy::cast_possible_truncation)]
                let min_score = val("min_score").as_f64().map(|n| n as f32);
                let out = self
                    .inner
                    .search(SearchRequest {
                        collection_id,
                        query,
                        top_k,
                        min_score,
                    })
                    .await
                    .map_err(map_rag_err)?;
                serde_json::to_value(&out)
                    .map_err(|e| McpError::Transport(format!("search response serialize: {e}")))?
            }
            "rag_ingest" => {
                let collection_id = str_arg("collection_id")?;
                let source_uri = str_arg("source_uri")?;
                let content = str_arg("content")?;
                let metadata = val("metadata");
                let out = self
                    .inner
                    .ingest(IngestRequest {
                        collection_id,
                        source_uri,
                        content,
                        metadata,
                    })
                    .await
                    .map_err(map_rag_err)?;
                serde_json::to_value(&out)
                    .map_err(|e| McpError::Transport(format!("ingest response serialize: {e}")))?
            }
            "rag_list_collections" => {
                let cs = self.inner.list_collections().await.map_err(map_rag_err)?;
                json!({ "collections": cs })
            }
            "rag_delete_document" => {
                let collection_id = str_arg("collection_id")?;
                let document_id = str_arg("document_id")?;
                self.inner
                    .delete_document(&collection_id, &document_id)
                    .await
                    .map_err(map_rag_err)?;
                json!({ "deleted": true })
            }
            other => {
                return Err(McpError::InvalidArgument(format!(
                    "unknown tool: {other} (expected rag_search/ingest/list_collections/delete_document)"
                )));
            }
        };

        let text = result_json.to_string();
        Ok(ToolResult {
            text: text.clone(),
            blocks: vec![ContentBlock::Text { text }],
            is_error: false,
        })
    }

    async fn shutdown(&self) -> McpResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::InMemoryRagClient;

    fn mk_adapter() -> RagMcpAdapter {
        let mem = Arc::new(InMemoryRagClient::new());
        mem.ensure_collection("notes", "Notes", Some("scratch"));
        RagMcpAdapter::new(mem)
    }

    #[tokio::test]
    async fn list_tools_returns_four() {
        let a = mk_adapter();
        let tools = a.list_tools().await.unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"rag_search"));
        assert!(names.contains(&"rag_ingest"));
        assert!(names.contains(&"rag_list_collections"));
        assert!(names.contains(&"rag_delete_document"));
    }

    #[tokio::test]
    async fn ingest_then_search_round_trip() {
        let a = mk_adapter();
        let ingest = a
            .call_tool(
                "rag_ingest",
                json!({
                    "collection_id": "notes",
                    "source_uri": "file:///x.md",
                    "content": "hello needle world"
                }),
            )
            .await
            .unwrap();
        assert!(!ingest.is_error);
        assert!(ingest.text.contains("document_id"));

        let search = a
            .call_tool(
                "rag_search",
                json!({ "collection_id": "notes", "query": "needle" }),
            )
            .await
            .unwrap();
        assert!(!search.is_error);
        let v: JsonValue = serde_json::from_str(&search.text).unwrap();
        let hits = v.get("hits").and_then(|h| h.as_array()).unwrap();
        assert_eq!(hits.len(), 1);
        let cit = &hits[0]["citation"];
        assert_eq!(cit["source_uri"], "file:///x.md");
        assert!(cit["score"].as_f64().unwrap() > 0.0);
    }

    #[tokio::test]
    async fn list_collections_round_trips_seeded_entry() {
        let a = mk_adapter();
        let r = a
            .call_tool("rag_list_collections", json!({}))
            .await
            .unwrap();
        let v: JsonValue = serde_json::from_str(&r.text).unwrap();
        let cs = v.get("collections").and_then(|c| c.as_array()).unwrap();
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0]["name"], "Notes");
    }

    #[tokio::test]
    async fn unknown_tool_surfaces_invalid_argument() {
        let a = mk_adapter();
        let err = a
            .call_tool("rag_nope", json!({}))
            .await
            .expect_err("should be InvalidArgument");
        assert!(matches!(err, McpError::InvalidArgument(_)));
    }
}
