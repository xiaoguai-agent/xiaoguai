//! rmcp → xiaoguai conversion for imported (external) MCP tools.
//!
//! T5 consult/execute: the one place an external server's self-declared
//! `annotations.readOnlyHint` (MCP spec; rmcp `ToolAnnotations.read_only_hint`)
//! survives into our [`ToolDescriptor`].
//!
//! #286: the hint is **not trusted by default**. A lying / compromised
//! external server can stamp `readOnlyHint: true` on a write tool, which
//! would let it through consult mode's read-only guarantee (both the
//! visibility subset and the `ConsultGate`). So unless the operator has
//! explicitly opted a server in (`trust_read_only_hints == true`, see
//! `supervisor::XIAOGUAI_MCP_TRUST_READ_ONLY_HINTS_ENV`), every external
//! tool maps to [`MutationHint::Write`] — consult mode excludes it.
//! With trust granted, `readOnlyHint == true` → [`MutationHint::Read`];
//! `false`/absent stays `Write` (fail-closed per plan §2.1).

use serde_json::Value as JsonValue;

use crate::types::{MutationHint, ToolDescriptor};

/// Convert one rmcp `tools/list` entry into our cross-transport descriptor.
///
/// `trust_read_only_hints` is the per-server operator opt-in (#286):
/// `false` (the default everywhere) ignores the server's `readOnlyHint`
/// entirely and classifies the tool as `Write`.
#[must_use]
pub fn descriptor_from_rmcp_tool(
    t: rmcp::model::Tool,
    trust_read_only_hints: bool,
) -> ToolDescriptor {
    let mutation_hint =
        mutation_hint_from_annotations(t.annotations.as_ref(), trust_read_only_hints);
    ToolDescriptor {
        name: t.name.into_owned(),
        description: t.description.map(std::borrow::Cow::into_owned),
        input_schema: serde_json::to_value(&*t.input_schema).unwrap_or(JsonValue::Null),
        mutation_hint,
    }
}

/// Map the MCP `readOnlyHint` annotation onto our hint.
///
/// #286: when `trust_read_only_hints` is `false` the annotation is ignored
/// — the result is always `Write` (the server's self-declaration is not a
/// security boundary). When the operator trusted the server, only an
/// explicit `true` yields `Read`; everything else (false, absent, no
/// annotations block at all) is `Write` — fail-closed per plan §2.1.
#[must_use]
pub fn mutation_hint_from_annotations(
    annotations: Option<&rmcp::model::ToolAnnotations>,
    trust_read_only_hints: bool,
) -> MutationHint {
    if !trust_read_only_hints {
        // #286: distrust by default — an external server's word alone never
        // earns consult-mode (read-only) eligibility.
        return MutationHint::Write;
    }
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

    // ── #286: default (untrusted) — the hint is ignored ─────────────────

    #[test]
    fn untrusted_read_only_hint_true_still_maps_to_write() {
        // The 🟠 finding in #286: a lying server stamps readOnlyHint:true on
        // a write tool. Without operator opt-in it must classify as Write,
        // i.e. consult mode excludes + denies it.
        let t = wire_tool(Some(json!({ "readOnlyHint": true })));
        let d = descriptor_from_rmcp_tool(t, false);
        assert_eq!(d.mutation_hint, MutationHint::Write);
        assert_eq!(d.name, "external_tool");
    }

    #[test]
    fn untrusted_absent_annotations_map_to_write() {
        let t = wire_tool(None);
        assert_eq!(
            descriptor_from_rmcp_tool(t, false).mutation_hint,
            MutationHint::Write
        );
    }

    // ── #286: per-server trust granted — spec mapping applies ───────────

    #[test]
    fn trusted_read_only_hint_true_maps_to_read() {
        let t = wire_tool(Some(json!({ "readOnlyHint": true })));
        let d = descriptor_from_rmcp_tool(t, true);
        assert_eq!(d.mutation_hint, MutationHint::Read);
        assert_eq!(d.name, "external_tool");
    }

    #[test]
    fn trusted_read_only_hint_false_maps_to_write() {
        let t = wire_tool(Some(json!({ "readOnlyHint": false })));
        assert_eq!(
            descriptor_from_rmcp_tool(t, true).mutation_hint,
            MutationHint::Write
        );
    }

    #[test]
    fn trusted_absent_annotations_map_to_write_fail_closed() {
        let t = wire_tool(None);
        assert_eq!(
            descriptor_from_rmcp_tool(t, true).mutation_hint,
            MutationHint::Write
        );
    }

    #[test]
    fn trusted_annotations_without_read_only_hint_map_to_write() {
        let t = wire_tool(Some(json!({ "destructiveHint": false })));
        assert_eq!(
            descriptor_from_rmcp_tool(t, true).mutation_hint,
            MutationHint::Write
        );
    }

    #[test]
    fn schema_and_description_survive_conversion() {
        let t = wire_tool(Some(json!({ "readOnlyHint": true })));
        let d = descriptor_from_rmcp_tool(t, false);
        assert_eq!(
            d.description.as_deref(),
            Some("from an external MCP server")
        );
        assert_eq!(d.input_schema["type"], "object");
    }
}
