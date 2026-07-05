//! MCP tool definitions exposed by [`crate::server::ExecServer`].
//!
//! One tool — `execute_javascript`. Separate trust boundary from the
//! Python sibling; gating belongs upstream in the agent loop under
//! `tool_call.execute_javascript`.

use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{ContentBlock, Tool, ToolAnnotations};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::exec::{ExecConfig, ExecError, ExecResult};
use crate::runtime::ExecBackend;

/// Canonical tool name. Callers in the agent loop must dispatch a `HotL`
/// `tool_call.execute_javascript` scope before invoking; see the runbook.
pub const EXECUTE_JAVASCRIPT: &str = "execute_javascript";

/// Default timeout when the caller doesn't supply one.
const DEFAULT_TIMEOUT_SECS: u64 = 30;
/// Hard upper bound — even if the caller asks for more we clamp to this
/// before further clamping to [`ExecConfig::max_timeout`].
const MAX_TIMEOUT_SECS: u64 = 60;

/// JSON-Schema for the `execute_javascript` input.
#[must_use]
pub fn execute_javascript_tool() -> Tool {
    let schema = json!({
        "type": "object",
        "properties": {
            "code": {
                "type": "string",
                "description": "JavaScript (ES2020+) source to execute. Stdin is empty. Stdout is captured up to 64 KB. Under Deno (default runtime) the snippet runs with --allow-none — no network, no filesystem, no env access."
            },
            "timeout_secs": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_TIMEOUT_SECS,
                "default": DEFAULT_TIMEOUT_SECS,
                "description": "Wall-clock cap. Hard-bounded by server config."
            }
        },
        "required": ["code"],
        "additionalProperties": false
    });
    let schema_obj = Arc::new(schema.as_object().cloned().unwrap_or_default());
    let annotations = ToolAnnotations::new()
        .read_only(false)
        .destructive(false)
        .idempotent(false)
        .open_world(false);
    Tool::new(
        EXECUTE_JAVASCRIPT,
        "[WRITE] Execute a self-contained JavaScript snippet in a fresh sandbox (no network, no persistent FS, hard memory + time caps). Default runtime is Deno with --allow-none; Node.js is opt-in via server config and pushes containment to the deploy layer. Returns stdout, stderr, exit code. Each call is a fresh process; nothing persists between calls. Sensitive in scope `tool_call.execute_javascript` — gate at the agent loop with a `HotL` policy before dispatch.",
        schema_obj,
    )
    .annotate(annotations)
}

/// Parsed `execute_javascript` arguments.
#[derive(Debug, Deserialize)]
pub struct ExecuteJavascriptArgs {
    pub code: String,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// Wire-shape of a successful tool result. Encoded as JSON inside an MCP
/// `ContentBlock::text` block so the LLM gets structured data, not a free-form
/// string it has to re-parse.
#[derive(Debug, Serialize, Deserialize)]
pub struct ExecuteJavascriptResultPayload {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub truncated: bool,
    pub timed_out: bool,
}

impl From<ExecResult> for ExecuteJavascriptResultPayload {
    fn from(r: ExecResult) -> Self {
        Self {
            exit_code: r.exit_code,
            stdout: r.stdout,
            stderr: r.stderr,
            duration_ms: r.duration_ms,
            truncated: r.truncated,
            timed_out: r.timed_out,
        }
    }
}

/// Adapt a parsed `ExecuteJavascriptArgs` through the supplied
/// [`ExecBackend`] and shape the outcome into MCP `Content` blocks.
/// Supervisor failures (runtime missing, fork failed) become
/// `is_error=true` text blocks; all other outcomes — including snippet
/// crashes and deadlines — are normal data results.
///
/// DEC-019: by accepting the backend as a parameter (instead of calling
/// `run_javascript` directly), this function works unchanged for L1
/// (process isolation + Deno `--allow-none`) and L3 (wasmtime +
/// QuickJS-WASM) tiers. The backend tier is chosen at
/// `ExecServer::new` / `ExecServer::with_backend` time.
pub async fn execute_javascript_call(
    backend: &dyn ExecBackend,
    _cfg: &ExecConfig,
    args: ExecuteJavascriptArgs,
) -> (Vec<ContentBlock>, bool) {
    let requested = args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
    let clamped = requested.min(MAX_TIMEOUT_SECS);
    let timeout = Duration::from_secs(clamped);

    match backend.run(&args.code, timeout).await {
        Ok(result) => {
            let payload = ExecuteJavascriptResultPayload::from(result);
            let json_text = serde_json::to_string(&payload)
                .unwrap_or_else(|e| format!(r#"{{"error":"serialize result: {e}"}}"#));
            (vec![ContentBlock::text(json_text)], false)
        }
        Err(ExecError::SnippetTooLarge(n)) => (
            vec![ContentBlock::text(format!(
                "snippet is {n} bytes; max 65536. Trim it or split into multiple calls."
            ))],
            true,
        ),
        Err(other) => (
            vec![ContentBlock::text(format!(
                "sandbox supervisor error: {other}"
            ))],
            true,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_schema_advertises_execute_javascript_with_write_marker() {
        let t = execute_javascript_tool();
        assert_eq!(t.name.as_ref(), EXECUTE_JAVASCRIPT);
        assert!(t
            .description
            .as_deref()
            .unwrap_or("")
            .starts_with("[WRITE]"));
        assert!(t
            .description
            .as_deref()
            .unwrap_or("")
            .contains("execute_javascript"));
    }

    #[test]
    fn execute_javascript_args_parse_with_defaults() {
        let v = json!({"code": "console.log(1)"});
        let parsed: ExecuteJavascriptArgs = serde_json::from_value(v).unwrap();
        assert_eq!(parsed.code, "console.log(1)");
        assert!(parsed.timeout_secs.is_none());
    }

    #[test]
    fn execute_javascript_args_reject_missing_code() {
        let v = json!({"timeout_secs": 5});
        let err = serde_json::from_value::<ExecuteJavascriptArgs>(v).unwrap_err();
        assert!(err.to_string().contains("code"));
    }

    #[test]
    fn timeout_request_is_clamped_to_max_in_schema() {
        // The JSON schema declares maximum=60; we don't rely on the
        // client to honour it (the supervisor clamps in
        // execute_javascript_call anyway), but assert the schema does
        // advertise the cap so well-behaved clients pre-validate.
        let t = execute_javascript_tool();
        let schema_json = serde_json::to_string(&*t.input_schema).unwrap();
        assert!(schema_json.contains("\"maximum\":60"));
    }
}
