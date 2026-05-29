//! MCP tool definitions exposed by [`crate::server::ExecServer`].
//!
//! Today there is exactly one tool — `execute_python`. JavaScript and other
//! runtimes will land as separate tools (and separate trust boundaries)
//! once Python proves stable in production.

use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{Content, Tool, ToolAnnotations};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::exec::{ExecConfig, ExecError, ExecResult};
use crate::runtime::ExecBackend;

/// Canonical tool name. Callers in the agent loop must dispatch a `HotL`
/// `tool_call.execute_python` scope before invoking; see the runbook.
pub const EXECUTE_PYTHON: &str = "execute_python";

/// Default timeout when the caller doesn't supply one.
const DEFAULT_TIMEOUT_SECS: u64 = 30;
/// Hard upper bound — even if the caller asks for more we clamp to this
/// before further clamping to [`ExecConfig::max_timeout`].
const MAX_TIMEOUT_SECS: u64 = 60;

/// JSON-Schema for the `execute_python` input.
#[must_use]
pub fn execute_python_tool() -> Tool {
    let schema = json!({
        "type": "object",
        "properties": {
            "code": {
                "type": "string",
                "description": "Python 3 source to execute. Stdin is empty. Stdout is captured up to 64 KB."
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
        EXECUTE_PYTHON,
        "[WRITE] Execute a self-contained Python 3 snippet in a fresh sandbox (no network, no persistent FS, hard memory + time caps). Returns stdout, stderr, exit code. Each call is a fresh process; nothing persists between calls. Sensitive in scope `tool_call.execute_python` — gate at the agent loop with a `HotL` policy before dispatch.",
        schema_obj,
    )
    .annotate(annotations)
}

/// Parsed `execute_python` arguments.
#[derive(Debug, Deserialize)]
pub struct ExecutePythonArgs {
    pub code: String,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// Wire-shape of a successful tool result. Encoded as JSON inside an MCP
/// `Content::text` block so the LLM gets structured data, not a free-form
/// string it has to re-parse.
#[derive(Debug, Serialize, Deserialize)]
pub struct ExecutePythonResultPayload {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub truncated: bool,
    pub timed_out: bool,
}

impl From<ExecResult> for ExecutePythonResultPayload {
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

/// Adapt a parsed `ExecutePythonArgs` through the supplied
/// [`ExecBackend`] and shape the outcome into MCP `Content` blocks.
/// Supervisor failures (python missing, fork failed) become
/// `is_error=true` text blocks; all other outcomes — including snippet
/// crashes and deadlines — are normal data results.
///
/// DEC-019: by accepting the backend as a parameter (instead of calling
/// `run_python` directly), this function works unchanged for L1 (process
/// isolation) and L3 (wasmtime) tiers. The backend tier is chosen at
/// `ExecServer::new` / `ExecServer::with_backend` time. The
/// `ExecConfig` argument is kept for caller convenience (e.g. surface
/// the configured `timeout_secs` ceiling) but the actual run-time
/// config lives inside the backend.
pub async fn execute_python_call(
    backend: &dyn ExecBackend,
    _cfg: &ExecConfig,
    args: ExecutePythonArgs,
) -> (Vec<Content>, bool) {
    let requested = args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
    let clamped = requested.min(MAX_TIMEOUT_SECS);
    let timeout = Duration::from_secs(clamped);

    match backend.run(&args.code, timeout).await {
        Ok(result) => {
            let payload = ExecutePythonResultPayload::from(result);
            let json_text = serde_json::to_string(&payload)
                .unwrap_or_else(|e| format!(r#"{{"error":"serialize result: {e}"}}"#));
            (vec![Content::text(json_text)], false)
        }
        Err(ExecError::SnippetTooLarge(n)) => (
            vec![Content::text(format!(
                "snippet is {n} bytes; max 65536. Trim it or split into multiple calls."
            ))],
            true,
        ),
        Err(other) => (
            vec![Content::text(format!("sandbox supervisor error: {other}"))],
            true,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_schema_advertises_execute_python() {
        let t = execute_python_tool();
        assert_eq!(t.name.as_ref(), EXECUTE_PYTHON);
        // Description must include the read/write marker so agents can
        // route by capability.
        assert!(t
            .description
            .as_deref()
            .unwrap_or("")
            .starts_with("[WRITE]"));
    }

    #[tokio::test]
    async fn happy_path_returns_structured_json_content() {
        let cfg = ExecConfig::default();
        let args = ExecutePythonArgs {
            code: "print('ok')".into(),
            timeout_secs: Some(5),
        };
        let backend = crate::runtime::ProcessL1Python::new(cfg.clone());
        let (contents, is_error) = execute_python_call(&backend, &cfg, args).await;
        assert!(!is_error);
        assert_eq!(contents.len(), 1);
        let text = match &contents[0].raw {
            rmcp::model::RawContent::Text(t) => t.text.clone(),
            other => panic!("expected text content, got {other:?}"),
        };
        let payload: ExecutePythonResultPayload =
            serde_json::from_str(&text).expect("payload must round-trip");
        assert_eq!(payload.exit_code, Some(0));
        assert_eq!(payload.stdout.trim(), "ok");
        assert!(!payload.timed_out);
    }

    #[tokio::test]
    async fn timeout_request_is_clamped() {
        let cfg = ExecConfig::default();
        let args = ExecutePythonArgs {
            code: "print('ok')".into(),
            // Caller asks for 999s; we clamp to MAX_TIMEOUT_SECS then to
            // ExecConfig.max_timeout (30s by default). This just asserts
            // the call returns; the clamp itself is exercised by the
            // exec.rs timeout tests.
            timeout_secs: Some(999),
        };
        let backend = crate::runtime::ProcessL1Python::new(cfg.clone());
        let (contents, is_error) = execute_python_call(&backend, &cfg, args).await;
        assert!(!is_error);
        assert!(!contents.is_empty());
    }
}
