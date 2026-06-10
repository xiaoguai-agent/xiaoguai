//! rmcp → xiaoguai conversion for imported (external) MCP tools.
//!
//! T5 consult/execute: the one place an external server's self-declared
//! `annotations.readOnlyHint` (MCP spec; rmcp `ToolAnnotations.read_only_hint`)
//! survives into our [`ToolDescriptor`]. `readOnlyHint == true` →
//! [`MutationHint::Read`]; `false`/absent → [`MutationHint::Write`]
//! (fail-closed — an unannotated external tool is consult-blocked).

use serde_json::Value as JsonValue;

use crate::types::{MutationHint, ToolDescriptor};

/// Convert one rmcp `tools/list` entry into our cross-transport descriptor.
#[must_use]
pub fn descriptor_from_rmcp_tool(t: rmcp::model::Tool) -> ToolDescriptor {
    let mutation_hint = mutation_hint_from_annotations(t.annotations.as_ref());
    ToolDescriptor {
        name: t.name.into_owned(),
        description: t.description.map(std::borrow::Cow::into_owned),
        input_schema: serde_json::to_value(&*t.input_schema).unwrap_or(JsonValue::Null),
        mutation_hint,
    }
}

/// Map the MCP `readOnlyHint` annotation onto our hint. Only an explicit
/// `true` yields `Read`; everything else (false, absent, no annotations
/// block at all) is `Write` — fail-closed per plan §2.1.
#[must_use]
pub fn mutation_hint_from_annotations(
    annotations: Option<&rmcp::model::ToolAnnotations>,
) -> MutationHint {
    match annotations.and_then(|a| a.read_only_hint) {
        Some(true) => MutationHint::Read,
        Some(false) | None => MutationHint::Write,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Deserialize a wire-shaped `tools/list` tool entry (camelCase JSON,
    /// exactly what an external server sends) into an rmcp `Tool`.
    fn wire_tool(annotations: Option<serde_json::Value>) -> rmcp::model::Tool {
        let mut v = json!({
            "name": "external_tool",
            "description": "from an external MCP server",
            "inputSchema": { "type": "object", "properties": {} }
        });
        if let (Some(a), Some(obj)) = (annotations, v.as_object_mut()) {
            obj.insert("annotations".into(), a);
        }
        serde_json::from_value(v).expect("wire payload deserializes as rmcp Tool")
    }

    #[test]
    fn read_only_hint_true_maps_to_read() {
        let t = wire_tool(Some(json!({ "readOnlyHint": true })));
        let d = descriptor_from_rmcp_tool(t);
        assert_eq!(d.mutation_hint, MutationHint::Read);
        assert_eq!(d.name, "external_tool");
    }

    #[test]
    fn read_only_hint_false_maps_to_write() {
        let t = wire_tool(Some(json!({ "readOnlyHint": false })));
        assert_eq!(
            descriptor_from_rmcp_tool(t).mutation_hint,
            MutationHint::Write
        );
    }

    #[test]
    fn absent_annotations_map_to_write_fail_closed() {
        let t = wire_tool(None);
        assert_eq!(
            descriptor_from_rmcp_tool(t).mutation_hint,
            MutationHint::Write
        );
    }

    #[test]
    fn annotations_without_read_only_hint_map_to_write() {
        let t = wire_tool(Some(json!({ "destructiveHint": false })));
        assert_eq!(
            descriptor_from_rmcp_tool(t).mutation_hint,
            MutationHint::Write
        );
    }

    #[test]
    fn schema_and_description_survive_conversion() {
        let t = wire_tool(Some(json!({ "readOnlyHint": true })));
        let d = descriptor_from_rmcp_tool(t);
        assert_eq!(
            d.description.as_deref(),
            Some("from an external MCP server")
        );
        assert_eq!(d.input_schema["type"], "object");
    }
}
