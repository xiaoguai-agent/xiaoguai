//! Minimal MCP server that speaks just enough of the protocol over stdio
//! to drive `StdioMcpClient` tests deterministically — no npx, no network.
//!
//! Spawned as a child process by the integration test; the test locates this
//! binary by env var `CARGO_BIN_EXE_mock-mcp-server` that cargo sets for any
//! `[[bin]]` defined in this crate.

use std::io::{self, BufRead, Write};

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines().map_while(Result::ok) {
        let req: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");

        // Notifications carry no id field — silently consume.
        if method.starts_with("notifications/") || id.is_null() {
            continue;
        }

        let resp = match method {
            "initialize" => serde_json::json!({
                "jsonrpc": "2.0", "id": id,
                "result": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "mock", "version": "0.1.0" }
                }
            }),
            "tools/list" => serde_json::json!({
                "jsonrpc": "2.0", "id": id,
                "result": {
                    "tools": [{
                        "name": "echo",
                        "description": "echoes its input",
                        "inputSchema": {
                            "type": "object",
                            "properties": { "msg": { "type": "string" } },
                            "required": ["msg"]
                        }
                    }]
                }
            }),
            "tools/call" => {
                let params = req.get("params");
                let tool = params
                    .and_then(|p| p.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                let args = params
                    .and_then(|p| p.get("arguments"))
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let text = match tool {
                    // SEC-03 probe: reports whether an env var is visible in
                    // THIS child process, so tests can assert the client
                    // scrubbed the host environment. Deliberately absent
                    // from `tools/list` — it's a test back door, not a tool.
                    "env_probe" => {
                        let key = args.get("key").and_then(|k| k.as_str()).unwrap_or("");
                        match std::env::var(key) {
                            Ok(v) => format!("set: {v}"),
                            Err(_) => "unset".to_string(),
                        }
                    }
                    _ => {
                        let msg = args.get("msg").and_then(|m| m.as_str()).unwrap_or("");
                        format!("echo: {msg}")
                    }
                };
                serde_json::json!({
                    "jsonrpc": "2.0", "id": id,
                    "result": {
                        "content": [{ "type": "text", "text": text }],
                        "isError": false
                    }
                })
            }
            _ => serde_json::json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32601, "message": format!("method not found: {method}") }
            }),
        };
        if writeln!(out, "{resp}").is_err() {
            break;
        }
        if out.flush().is_err() {
            break;
        }
    }
}
