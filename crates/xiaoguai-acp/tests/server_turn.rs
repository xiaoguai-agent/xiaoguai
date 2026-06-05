//! End-to-end protocol tests over an in-memory duplex pair, driving the real
//! [`serve`] dispatch loop with a deterministic stub delegate. Verifies the P2
//! behaviours `BEH-ACP-001/002/003`.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use xiaoguai_acp::acp::{ContentBlock, ContentChunk, SessionUpdate, StopReason};
use xiaoguai_acp::transport::{LineReader, LineWriter};
use xiaoguai_acp::{serve, AcpDelegate, UpdateSink};

/// A delegate that streams two assistant chunks then ends — unless cancelled,
/// in which case it waits for the token and reports `Cancelled`.
struct StubDelegate {
    honor_cancel: bool,
}

#[async_trait]
impl AcpDelegate for StubDelegate {
    async fn prompt(
        &self,
        _session_id: &str,
        prompt_text: String,
        sink: UpdateSink,
        cancel: CancellationToken,
    ) -> StopReason {
        if self.honor_cancel {
            cancel.cancelled().await;
            return StopReason::Cancelled;
        }
        sink.send(chunk(&format!("echo: {prompt_text}")));
        sink.send(chunk("done"));
        StopReason::EndTurn
    }
}

fn chunk(text: &str) -> SessionUpdate {
    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from(text.to_string())))
}

/// A connected client: writes requests into the server's reader, reads the
/// server's replies. Two independent duplex channels model real stdin/stdout.
struct Client {
    to_server: LineWriter<tokio::io::DuplexStream>,
    from_server: LineReader<tokio::io::DuplexStream>,
}

impl Client {
    fn spawn(honor_cancel: bool) -> Self {
        let (c2s_client, c2s_server) = tokio::io::duplex(8 * 1024);
        let (s2c_server, s2c_client) = tokio::io::duplex(8 * 1024);
        let delegate: Arc<dyn AcpDelegate> = Arc::new(StubDelegate { honor_cancel });
        tokio::spawn(async move {
            let _ = serve(delegate, c2s_server, s2c_server).await;
        });
        Self {
            to_server: LineWriter::new(c2s_client),
            from_server: LineReader::new(s2c_client),
        }
    }

    async fn send(&self, value: Value) {
        self.to_server.write_message(&value).await.unwrap();
    }

    async fn recv(&mut self) -> Value {
        let line = self
            .from_server
            .next_message()
            .await
            .unwrap()
            .expect("server closed unexpectedly");
        serde_json::from_str(&line).unwrap()
    }
}

#[tokio::test]
async fn initialize_handshake() {
    // BEH-ACP-001
    let mut client = Client::spawn(false);
    client
        .send(json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": 1, "clientCapabilities": {} }
        }))
        .await;
    let resp = client.recv().await;
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["protocolVersion"], 1);
    assert_eq!(resp["result"]["agentInfo"]["name"], "xiaoguai");
}

#[tokio::test]
async fn new_session_then_prompt_turn() {
    // BEH-ACP-002
    let mut client = Client::spawn(false);
    client
        .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "session/new",
                      "params": { "cwd": "/tmp", "mcpServers": [] } }))
        .await;
    let new_resp = client.recv().await;
    let session_id = new_resp["result"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    client
        .send(json!({
            "jsonrpc": "2.0", "id": 2, "method": "session/prompt",
            "params": { "sessionId": session_id, "prompt": [{ "type": "text", "text": "hi" }] }
        }))
        .await;

    // Two session/update notifications, in order, then the response.
    let u1 = client.recv().await;
    assert_eq!(u1["method"], "session/update");
    assert_eq!(u1["params"]["sessionId"], session_id);
    assert_eq!(
        u1["params"]["update"]["sessionUpdate"],
        "agent_message_chunk"
    );
    assert_eq!(u1["params"]["update"]["content"]["text"], "echo: hi");

    let u2 = client.recv().await;
    assert_eq!(u2["params"]["update"]["content"]["text"], "done");

    let resp = client.recv().await;
    assert_eq!(resp["id"], 2);
    assert_eq!(resp["result"]["stopReason"], "end_turn");
}

#[tokio::test]
async fn cancel_ends_turn_with_cancelled() {
    // BEH-ACP-003
    let mut client = Client::spawn(true);
    client
        .send(json!({ "jsonrpc": "2.0", "id": 1, "method": "session/new",
                      "params": { "cwd": "/tmp", "mcpServers": [] } }))
        .await;
    let session_id = client.recv().await["result"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    client
        .send(json!({
            "jsonrpc": "2.0", "id": 2, "method": "session/prompt",
            "params": { "sessionId": session_id, "prompt": [{ "type": "text", "text": "slow" }] }
        }))
        .await;
    // The stub blocks until cancelled; send the cancel notification.
    client
        .send(json!({ "jsonrpc": "2.0", "method": "session/cancel",
                      "params": { "sessionId": session_id } }))
        .await;

    let resp = client.recv().await;
    assert_eq!(resp["id"], 2);
    assert_eq!(resp["result"]["stopReason"], "cancelled");
}

#[tokio::test]
async fn unknown_method_returns_method_not_found() {
    let mut client = Client::spawn(false);
    client
        .send(json!({ "jsonrpc": "2.0", "id": 7, "method": "session/teleport", "params": {} }))
        .await;
    let resp = client.recv().await;
    assert_eq!(resp["id"], 7);
    assert_eq!(resp["error"]["code"], -32601);
}

#[tokio::test]
async fn prompt_for_unknown_session_is_invalid_params() {
    let mut client = Client::spawn(false);
    client
        .send(json!({
            "jsonrpc": "2.0", "id": 9, "method": "session/prompt",
            "params": { "sessionId": "ghost", "prompt": [{ "type": "text", "text": "hi" }] }
        }))
        .await;
    let resp = client.recv().await;
    assert_eq!(resp["id"], 9);
    assert_eq!(resp["error"]["code"], -32602);
}

#[tokio::test]
async fn malformed_line_yields_parse_error() {
    let mut client = Client::spawn(false);
    // Write a raw non-JSON line directly through the transport.
    client
        .to_server
        .write_message(&json!("this is valid json but not an object with method"))
        .await
        .unwrap();
    let resp = client.recv().await;
    assert_eq!(resp["error"]["code"], -32700);
}
