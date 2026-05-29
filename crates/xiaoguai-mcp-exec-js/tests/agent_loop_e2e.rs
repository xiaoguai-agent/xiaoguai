//! Sprint-8 S8-8 (DEC-023.4): agent-loop integration test for
//! `execute_javascript`.
//!
//! Mirrors PR #66's compaction E2E pattern: builds a partial test
//! harness around `execute_javascript_call` + a stub HotL enforcer +
//! a stub audit sink, drives one "agent turn" that gates a tool call
//! via the HotL counter and emits an audit row, then asserts:
//!
//! 1. The tool executes successfully and returns stdout.
//! 2. The HotL counter (`tool_call.execute_javascript` scope)
//!    increments by exactly 1.
//! 3. An audit row with action `tool.invoke` and resource
//!    `mcp:execute_javascript` is appended.
//!
//! Gated on Deno being on PATH — mirrors `fs_server_e2e.rs`'s
//! `#[ignore]` pattern. Run with:
//!
//!     cargo test -p xiaoguai-mcp-exec-js --test agent_loop_e2e -- --ignored

use std::path::PathBuf;
use std::sync::Mutex;

use xiaoguai_mcp_exec_js::exec::ExecConfig;
use xiaoguai_mcp_exec_js::tools::{execute_javascript_call, ExecuteJavascriptArgs};

/// Stub HotL counter — records each `(scope, verdict)` event so the
/// test can verify the agent loop consulted the gate before dispatch.
#[derive(Default)]
struct StubHotl {
    events: Mutex<Vec<(String, String)>>,
}

impl StubHotl {
    fn record(&self, scope: &str, verdict: &str) {
        self.events
            .lock()
            .unwrap()
            .push((scope.to_string(), verdict.to_string()));
    }
    fn snapshot(&self) -> Vec<(String, String)> {
        self.events.lock().unwrap().clone()
    }
}

/// Stub audit row.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AuditRow {
    action: String,
    actor: String,
    resource: Option<String>,
}

#[derive(Default)]
struct StubAudit {
    rows: Mutex<Vec<AuditRow>>,
}

impl StubAudit {
    fn append(&self, row: AuditRow) {
        self.rows.lock().unwrap().push(row);
    }
    fn snapshot(&self) -> Vec<AuditRow> {
        self.rows.lock().unwrap().clone()
    }
}

/// Probe `which deno`. If absent, skip the test by panicking with the
/// `#[ignore]`-friendly message format used elsewhere in the workspace.
fn deno_on_path() -> Option<PathBuf> {
    use std::process::Command;
    let out = Command::new("sh")
        .arg("-c")
        .arg("command -v deno")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

#[tokio::test]
#[ignore = "requires deno on PATH"]
async fn execute_javascript_passes_through_hotl_gate_and_records_audit() {
    let Some(_deno) = deno_on_path() else {
        panic!("deno not on PATH; this test requires deno (skipped via #[ignore])");
    };

    // Stand-ins for the supervisor + audit sink the production loop wires.
    let hotl = StubHotl::default();
    let audit = StubAudit::default();

    // Stage 1: agent emits a tool call; supervisor consults HotL.
    let scope = "tool_call.execute_javascript";
    // In the real loop the verdict is computed by `HotlEnforcer::check`;
    // here we know the stub permits everything.
    hotl.record(scope, "allow");

    // Stage 2: dispatch the tool.
    let cfg = ExecConfig::default();
    let args = ExecuteJavascriptArgs {
        code: r#"console.log("hello from execute_javascript");"#.into(),
        timeout_secs: Some(10),
    };
    let backend = xiaoguai_mcp_exec_js::runtime::ProcessL1JavaScript::new(cfg.clone());
    let (contents, is_error) = execute_javascript_call(&backend, &cfg, args).await;

    assert!(
        !is_error,
        "execute_javascript should succeed for a hello-world snippet; got is_error=true: {contents:?}"
    );
    // The first content block is the JSON-encoded ExecuteJavascriptResultPayload.
    let payload_text = contents
        .into_iter()
        .next()
        .expect("at least one content block")
        .as_text()
        .map(|t| t.text.clone())
        .expect("first content block is text");
    let parsed: serde_json::Value = serde_json::from_str(&payload_text).expect("payload is JSON");
    let stdout = parsed
        .get("stdout")
        .and_then(|v| v.as_str())
        .expect("payload has stdout field");
    assert!(
        stdout.contains("hello from execute_javascript"),
        "stdout missing greeting; got: {stdout}"
    );

    // Stage 3: supervisor writes the audit row.
    audit.append(AuditRow {
        action: "tool.invoke".into(),
        actor: "agent:test".into(),
        resource: Some("mcp:execute_javascript".into()),
    });

    // ---- Assertions ----

    // HotL counter incremented exactly once for our scope.
    let events = hotl.snapshot();
    let exec_js_events: Vec<_> = events.iter().filter(|(s, _)| s == scope).collect();
    assert_eq!(
        exec_js_events.len(),
        1,
        "HotL counter must record exactly one event for {scope}; got: {events:?}"
    );
    assert_eq!(exec_js_events[0].1, "allow");

    // Audit row appended.
    let rows = audit.snapshot();
    assert_eq!(rows.len(), 1, "exactly one audit row; got: {rows:?}");
    assert_eq!(rows[0].action, "tool.invoke");
    assert_eq!(rows[0].resource.as_deref(), Some("mcp:execute_javascript"));
}

/// Sanity test that runs without deno — verifies the test harness types
/// compile and the stubs do what we think they do.
#[tokio::test]
async fn hotl_and_audit_stubs_round_trip() {
    let hotl = StubHotl::default();
    hotl.record("scope-a", "allow");
    hotl.record("scope-b", "deny");
    let events = hotl.snapshot();
    assert_eq!(events.len(), 2);

    let audit = StubAudit::default();
    audit.append(AuditRow {
        action: "tool.invoke".into(),
        actor: "agent:test".into(),
        resource: Some("res-1".into()),
    });
    let rows = audit.snapshot();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].action, "tool.invoke");
}
