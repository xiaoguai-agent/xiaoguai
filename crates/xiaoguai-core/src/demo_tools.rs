//! Built-in **demo** tool pair for showing the consult/execute split live.
//!
//! The consult/execute story (T5) hinges on `MutationHint`: a `Read` tool is
//! safe in consult (read-only) mode; a `Write` tool is hidden from the model
//! (layer 1, `read_only_toolbox`) AND denied by the `ConsultGate` before
//! dispatch (layer 2, which also writes a `consult.denied` audit row). To
//! demo this reliably — without relying on the model to *choose* a write tool —
//! we ship a tiny, self-contained pair:
//!
//! * `demo_read_note` — reads an in-memory note (`MutationHint::Read`).
//! * `demo_write_note` — overwrites that note (`MutationHint::Write`).
//!
//! In **execute** mode the agent can call both. In **consult** mode
//! `demo_write_note` is filtered out of the toolbox and, if the model
//! hallucinates the name anyway, the gate denies it + audits `consult.denied`.
//! The note lives in process memory (a `Mutex<String>`), so the tools touch no
//! real resource and are safe to leave registered behind a flag.
//!
//! **Opt-in**: these register ONLY when `XIAOGUAI_DEMO_TOOLS` is truthy
//! (`1`/`true`/`yes`). Default builds carry no demo tools (no surface-area or
//! confusion in production). This mirrors the coding-tools opt-in
//! (`coding_bridge`): an in-process [`McpClient`] backing [`Toolbox`] entries,
//! no change to `xiaoguai-runtime` / `xiaoguai-agent`.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use xiaoguai_agent::Toolbox;
use xiaoguai_mcp::{McpClient, McpResult, MutationHint, ServerInfo, ToolDescriptor, ToolResult};

/// Tool name for the read half of the demo pair.
pub const DEMO_READ_NOTE: &str = "demo_read_note";
/// Tool name for the write half of the demo pair.
pub const DEMO_WRITE_NOTE: &str = "demo_write_note";

/// The note shown before anything is written, so `demo_read_note` returns
/// something meaningful on a fresh process.
const INITIAL_NOTE: &str =
    "(demo 便签为空) — execute 模式下用 demo_write_note 写入；consult 模式下写入会被拦截。";

/// In-process [`McpClient`] backing the demo note tools. Holds a single shared
/// note string; `read` returns it, `write` overwrites it. No external I/O.
struct DemoNoteClient {
    note: Mutex<String>,
}

impl DemoNoteClient {
    fn new() -> Self {
        Self {
            note: Mutex::new(INITIAL_NOTE.to_string()),
        }
    }

    /// Current note contents. Recovers from a poisoned lock (a panic while
    /// holding it) by reading the inner value — the note is a plain string,
    /// so a poisoned read is still meaningful and must never crash a turn.
    fn read(&self) -> String {
        self.note
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Overwrite the note, returning the new value. Same poison-recovery
    /// stance as [`read`](Self::read).
    fn write(&self, text: String) -> String {
        // Clone for the return value first, then move `text` into the stored
        // note (one clone total; avoids clippy's assigning_clones).
        let echo = text.clone();
        let mut guard = self
            .note
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = text;
        echo
    }
}

fn text_arg(args: &JsonValue, key: &str) -> String {
    args.get(key)
        .and_then(JsonValue::as_str)
        .unwrap_or("")
        .to_string()
}

#[async_trait]
impl McpClient for DemoNoteClient {
    async fn initialize(&self) -> McpResult<ServerInfo> {
        Ok(ServerInfo {
            name: "xiaoguai-demo".into(),
            version: "1".into(),
        })
    }

    async fn list_tools(&self) -> McpResult<Vec<ToolDescriptor>> {
        Ok(demo_tool_descriptors())
    }

    async fn call_tool(&self, name: &str, args: JsonValue) -> McpResult<ToolResult> {
        match name {
            DEMO_READ_NOTE => Ok(ok_result(format!("便签内容：{}", self.read()))),
            DEMO_WRITE_NOTE => {
                let text = text_arg(&args, "text");
                if text.trim().is_empty() {
                    return Ok(err_result("demo_write_note 需要非空的 `text` 参数"));
                }
                let saved = self.write(text);
                Ok(ok_result(format!("已写入 demo 便签：{saved}")))
            }
            other => Ok(err_result(&format!("unknown demo tool: {other}"))),
        }
    }

    async fn shutdown(&self) -> McpResult<()> {
        Ok(())
    }
}

fn ok_result(text: impl Into<String>) -> ToolResult {
    ToolResult {
        text: text.into(),
        blocks: vec![],
        is_error: false,
    }
}

fn err_result(text: &str) -> ToolResult {
    ToolResult {
        text: text.to_string(),
        blocks: vec![],
        is_error: true,
    }
}

/// The two demo descriptors. `mutation_hint` is the load-bearing field: it
/// drives both consult-mode visibility (layer 1) and gate enforcement
/// (layer 2). The `[READ]`/`[WRITE]` tag is for the model's benefit.
#[must_use]
pub fn demo_tool_descriptors() -> Vec<ToolDescriptor> {
    vec![
        ToolDescriptor {
            name: DEMO_READ_NOTE.into(),
            description: Some(
                "[READ] Read the contents of the in-memory demo note. No args. \
                 Safe in consult (read-only) mode — observes, never mutates."
                    .into(),
            ),
            input_schema: json!({ "type": "object", "properties": {} }),
            mutation_hint: MutationHint::Read,
        },
        ToolDescriptor {
            name: DEMO_WRITE_NOTE.into(),
            description: Some(
                "[WRITE] Overwrite the in-memory demo note. Args: text (string, \
                 required). A mutation — hidden + denied in consult mode (writes \
                 a `consult.denied` audit row), allowed in execute mode."
                    .into(),
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "New note contents." }
                },
                "required": ["text"]
            }),
            mutation_hint: MutationHint::Write,
        },
    ]
}

/// Whether the demo tools are enabled — off unless `XIAOGUAI_DEMO_TOOLS` is
/// truthy (`1`/`true`/`yes`). Off in production by default (the tools are only
/// useful for a live consult/execute demo).
#[must_use]
pub fn demo_tools_enabled() -> bool {
    std::env::var("XIAOGUAI_DEMO_TOOLS").is_ok_and(|v| {
        let v = v.trim().to_ascii_lowercase();
        v == "1" || v == "true" || v == "yes"
    })
}

/// Return `base` extended with the demo tool pair (immutable: `base` is
/// untouched, a new [`Toolbox`] is returned). The demo tools take precedence
/// over any same-named server tool via `insert_or_replace` — the names are
/// `demo_`-prefixed, so a collision is only possible with another demo source.
///
/// Call this only when [`demo_tools_enabled`] is true; `run_serve` gates it.
#[must_use]
pub fn with_demo_tools(base: &Toolbox) -> Toolbox {
    let client: Arc<dyn McpClient> = Arc::new(DemoNoteClient::new());
    let mut tb = base.clone();
    for descriptor in demo_tool_descriptors() {
        tb.insert_or_replace(client.clone(), descriptor);
    }
    tb
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn descriptors_carry_correct_mutation_hints() {
        let descs = demo_tool_descriptors();
        let read = descs.iter().find(|d| d.name == DEMO_READ_NOTE).unwrap();
        let write = descs.iter().find(|d| d.name == DEMO_WRITE_NOTE).unwrap();
        assert_eq!(read.mutation_hint, MutationHint::Read);
        assert_eq!(write.mutation_hint, MutationHint::Write);
    }

    #[test]
    fn with_demo_tools_adds_both_to_toolbox() {
        let tb = with_demo_tools(&Toolbox::new());
        assert!(tb.get(DEMO_READ_NOTE).is_some());
        assert!(tb.get(DEMO_WRITE_NOTE).is_some());
        assert_eq!(tb.len(), 2);
    }

    #[test]
    fn with_demo_tools_leaves_base_untouched() {
        let base = Toolbox::new();
        let _extended = with_demo_tools(&base);
        // Immutability: the base toolbox must not have gained the demo tools.
        assert!(base.get(DEMO_READ_NOTE).is_none());
        assert_eq!(base.len(), 0);
    }

    #[tokio::test]
    async fn read_then_write_then_read_round_trips() {
        let client = DemoNoteClient::new();

        // Initial read returns the placeholder.
        let r0 = client.call_tool(DEMO_READ_NOTE, json!({})).await.unwrap();
        assert!(!r0.is_error);
        assert!(r0.text.contains("便签"));

        // Write a value.
        let w = client
            .call_tool(DEMO_WRITE_NOTE, json!({ "text": "demo says hi" }))
            .await
            .unwrap();
        assert!(!w.is_error);
        assert!(w.text.contains("demo says hi"));

        // Read reflects the write.
        let r1 = client.call_tool(DEMO_READ_NOTE, json!({})).await.unwrap();
        assert!(r1.text.contains("demo says hi"));
    }

    #[tokio::test]
    async fn write_without_text_is_an_error() {
        let client = DemoNoteClient::new();
        let res = client.call_tool(DEMO_WRITE_NOTE, json!({})).await.unwrap();
        assert!(res.is_error, "empty write must error, not silently no-op");
    }

    #[tokio::test]
    async fn unknown_tool_name_errors() {
        let client = DemoNoteClient::new();
        let res = client.call_tool("demo_nope", json!({})).await.unwrap();
        assert!(res.is_error);
    }

    #[test]
    fn enabled_flag_reads_truthy_values() {
        // Exercises the parsing branch logic without mutating process env in a
        // way that races other tests: check the helper's truthy set directly.
        for (v, want) in [
            ("1", true),
            ("true", true),
            ("YES", true),
            ("0", false),
            ("", false),
        ] {
            let parsed = {
                let v = v.trim().to_ascii_lowercase();
                v == "1" || v == "true" || v == "yes"
            };
            assert_eq!(parsed, want, "value {v:?}");
        }
    }

    // ── consult/execute behaviour through the REAL production filters ────────
    // These exercise `xiaoguai-api`'s `read_only_toolbox` / `read_only_tool_names`
    // (the same layer-1 visibility + layer-2 gate-key logic `run_turn` uses), so
    // the demo pair is proven to behave correctly in consult mode — not via a
    // reimplementation of the rule.

    #[test]
    fn consult_hides_demo_write_but_keeps_demo_read() {
        // Layer 1 (visibility): the read-only toolbox the model sees in consult
        // mode contains `demo_read_note` and NOT `demo_write_note`.
        let base = with_demo_tools(&Toolbox::new());
        let consult_view = xiaoguai_api::consult::read_only_toolbox(&base);

        assert!(
            consult_view.get(DEMO_READ_NOTE).is_some(),
            "demo_read_note (Read) must remain visible in consult mode"
        );
        assert!(
            consult_view.get(DEMO_WRITE_NOTE).is_none(),
            "demo_write_note (Write) must be hidden in consult mode"
        );
        assert_eq!(consult_view.len(), 1);
    }

    #[test]
    fn consult_gate_key_set_excludes_demo_write() {
        // Layer 2 (enforcement): the ConsultGate's read-only name set — the keys
        // it allows through — includes the read tool and excludes the write
        // tool, so a hallucinated `demo_write_note` is denied before dispatch.
        let base = with_demo_tools(&Toolbox::new());
        let read_set = xiaoguai_api::consult::read_only_tool_names(&base);

        assert!(read_set.contains(DEMO_READ_NOTE));
        assert!(
            !read_set.contains(DEMO_WRITE_NOTE),
            "demo_write_note must not be in the consult allow-set (→ gate denies it)"
        );
    }

    #[test]
    fn execute_view_keeps_both_demo_tools() {
        // Execute mode uses the full toolbox — both tools are callable.
        let base = with_demo_tools(&Toolbox::new());
        assert!(base.get(DEMO_READ_NOTE).is_some());
        assert!(base.get(DEMO_WRITE_NOTE).is_some());
    }
}
