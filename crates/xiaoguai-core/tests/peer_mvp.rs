//! v1.1.5a — Peer MVP integration test.
//!
//! Spins two xiaoguai-core peers in-process on `127.0.0.1:0` ports:
//!   * **specialist** — publishes its `Toolbox` at `/v1/mcp/serve`
//!     (`mcp_publish_enabled = true`). Its `Toolbox` holds one tool,
//!     `summarize_webpage`, backed by an in-process `McpClient` stub.
//!   * **front-door** — its `Toolbox` is populated by connecting an
//!     `HttpMcpClient` to the specialist's published endpoint and
//!     copying every `list_tools()` result into the catalogue.
//!
//! Then exercises the two assertions in the v1.1.5a brief:
//!   1. The front-door's `Toolbox` lists the specialist's tool.
//!   2. An end-to-end `POST /v1/sessions/{id}/messages` against the
//!      front-door triggers a remote `call_tool` that completes
//!      successfully and the specialist's reply ends up in the
//!      front-door's persisted message history.
//!
//! No Docker, no PG, no Valkey — only two `tokio` TCP listeners on
//! ephemeral ports.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{json, Value as JsonValue};
use xiaoguai_agent::{AgentConfig, Toolbox};
use xiaoguai_api::{serve_with_state, AppState, CancelRegistry};
use xiaoguai_llm::mock::ScriptStep;
use xiaoguai_llm::{LlmBackend, MockBackend, ToolCallSpec};
use xiaoguai_mcp::{
    ContentBlock, HttpClientConfig, HttpMcpClient, McpClient, McpResult,
    ServerInfo as McpServerInfo, ToolDescriptor, ToolResult,
};
use xiaoguai_storage::repositories::{MessageRepository, RepoError, RepoResult, SessionRepository};
use xiaoguai_types::{Message, Session};

// ---------------------------------------------------------------------------
// Minimal in-memory repos — same shape as xiaoguai-api/tests/common but
// duplicated here so this crate's integration test stays self-contained
// (Cargo doesn't let one crate import another crate's `tests/common`).
// ---------------------------------------------------------------------------

#[derive(Default)]
struct InMemorySessionRepo {
    inner: Mutex<HashMap<String, Session>>,
}

impl InMemorySessionRepo {
    fn arc() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

#[async_trait]
impl SessionRepository for InMemorySessionRepo {
    async fn create(&self, _tenant: Option<&str>, session: &Session) -> RepoResult<()> {
        let mut g = self.inner.lock();
        if g.contains_key(session.id.as_str()) {
            return Err(RepoError::DuplicateKey("duplicate session id".into()));
        }
        g.insert(session.id.to_string(), session.clone());
        Ok(())
    }

    async fn find_by_id(&self, _tenant: Option<&str>, id: &str) -> RepoResult<Option<Session>> {
        Ok(self.inner.lock().get(id).cloned())
    }

    async fn list_by_user(
        &self,
        _tenant: Option<&str>,
        user_id: &str,
        limit: i64,
        offset: i64,
    ) -> RepoResult<Vec<Session>> {
        let mut rows: Vec<Session> = self
            .inner
            .lock()
            .values()
            .filter(|s| s.user_id.as_str() == user_id)
            .cloned()
            .collect();
        rows.sort_by_key(|s| s.created_at);
        let offset = usize::try_from(offset.max(0)).unwrap_or(0);
        let limit = usize::try_from(limit.max(0)).unwrap_or(0);
        Ok(rows.into_iter().skip(offset).take(limit).collect())
    }

    async fn touch(&self, _tenant: Option<&str>, id: &str) -> RepoResult<()> {
        let mut g = self.inner.lock();
        if let Some(s) = g.get_mut(id) {
            s.updated_at = chrono::Utc::now();
        }
        Ok(())
    }

    async fn archive(&self, _tenant: Option<&str>, id: &str) -> RepoResult<()> {
        let mut g = self.inner.lock();
        if let Some(s) = g.get_mut(id) {
            s.status = xiaoguai_types::SessionStatus::Archived;
        }
        Ok(())
    }

    async fn delete(&self, _tenant: Option<&str>, id: &str) -> RepoResult<()> {
        self.inner.lock().remove(id);
        Ok(())
    }
}

#[derive(Default)]
struct InMemoryMessageRepo {
    inner: Mutex<HashMap<String, Vec<Message>>>,
}

impl InMemoryMessageRepo {
    fn arc() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn snapshot(&self, session_id: &str) -> Vec<Message> {
        self.inner
            .lock()
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }
}

#[async_trait]
impl MessageRepository for InMemoryMessageRepo {
    async fn append(&self, _tenant: Option<&str>, message: &Message) -> RepoResult<()> {
        self.inner
            .lock()
            .entry(message.session_id.to_string())
            .or_default()
            .push(message.clone());
        Ok(())
    }

    async fn list_by_session(
        &self,
        _tenant: Option<&str>,
        session_id: &str,
        limit: i64,
        offset: i64,
    ) -> RepoResult<Vec<Message>> {
        let rows = self
            .inner
            .lock()
            .get(session_id)
            .cloned()
            .unwrap_or_default();
        let offset = usize::try_from(offset.max(0)).unwrap_or(0);
        let limit = usize::try_from(limit.max(0)).unwrap_or(0);
        Ok(rows.into_iter().skip(offset).take(limit).collect())
    }

    async fn count_by_session(&self, _tenant: Option<&str>, session_id: &str) -> RepoResult<i64> {
        Ok(self
            .inner
            .lock()
            .get(session_id)
            .map_or(0, |v| i64::try_from(v.len()).unwrap_or(i64::MAX)))
    }

    async fn delete_by_session(&self, _tenant: Option<&str>, session_id: &str) -> RepoResult<u64> {
        let removed = self
            .inner
            .lock()
            .remove(session_id)
            .map_or(0, |v| u64::try_from(v.len()).unwrap_or(u64::MAX));
        Ok(removed)
    }
}

// ---------------------------------------------------------------------------
// Specialist-side: an MCP backend that hosts the `summarize_webpage`
// tool. In a real specialist, this is where the operator would either
// wrap the specialist's *own* ReactAgent behind the tool, or register a
// stdio MCP server (RAG, GitHub, whatever the specialist is for). For
// the test we keep it deterministic — every call returns a canned
// 3-bullet "summary" of whatever URL the front-door asked about.
// ---------------------------------------------------------------------------

const SPECIALIST_TOOL_NAME: &str = "summarize_webpage";
const SPECIALIST_REPLY_PREFIX: &str = "[specialist] summary of";

struct SummariserBackend;

#[async_trait]
impl McpClient for SummariserBackend {
    async fn initialize(&self) -> McpResult<McpServerInfo> {
        Ok(McpServerInfo {
            name: "summariser-backend".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        })
    }
    async fn list_tools(&self) -> McpResult<Vec<ToolDescriptor>> {
        Ok(vec![])
    }
    async fn call_tool(&self, name: &str, args: JsonValue) -> McpResult<ToolResult> {
        let url = args
            .get("url")
            .and_then(JsonValue::as_str)
            .unwrap_or("<unknown>");
        let text = format!("{SPECIALIST_REPLY_PREFIX} {url} (via {name})");
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

fn specialist_toolbox() -> Toolbox {
    let client: Arc<dyn McpClient> = Arc::new(SummariserBackend);
    let descriptors = vec![ToolDescriptor {
        name: SPECIALIST_TOOL_NAME.into(),
        description: Some("Summarise a webpage in three bullets.".into()),
        input_schema: json!({
            "type": "object",
            "properties": { "url": { "type": "string" } },
            "required": ["url"]
        }),
    }];
    Toolbox::from_server(client, descriptors).expect("specialist toolbox")
}

fn build_state(
    toolbox: Toolbox,
    backend: Arc<dyn LlmBackend>,
    publish: bool,
) -> (AppState, Arc<InMemorySessionRepo>, Arc<InMemoryMessageRepo>) {
    let sessions = InMemorySessionRepo::arc();
    let messages = InMemoryMessageRepo::arc();
    let state = AppState {
        sessions: sessions.clone(),
        messages: messages.clone(),
        backend,
        toolbox: Arc::new(toolbox),
        agent_defaults: AgentConfig::new("mock-model"),
        cancels: Arc::new(CancelRegistry::new()),
        mcp_servers: None,
        auth: None,
        authz: None,
        tenants: None,
        rate_limiter: None,
        audit: None,
        audit_verifier: None,
        mcp_publish_enabled: publish,
        mcp_supervisor: None,
        today: None,
        eval: None,
        webhook_pusher: None,
        nl_job_compiler: None,
        job_upserter: None,
        session_forker: None,
        usage_reader: None,
        webhook_token_validator: None,
        webhook_token_admin: None,
        scheduler_jobs_reader: None,
        rate_limit_state: None,
    };
    (state, sessions, messages)
}

async fn spawn(state: AppState) -> std::net::SocketAddr {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let (bound, fut) = serve_with_state(addr, state).await.expect("bind");
    tokio::spawn(fut);
    // Let the listener begin accepting before the next connect attempt.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    bound
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Bring up the specialist + front-door peers, point the front-door's
/// `Toolbox` at the specialist via `HttpMcpClient`, and assert the
/// specialist's tool is visible to the front-door's catalogue.
///
/// This covers the brief's assertion #2 ("Front-door's Toolbox lists
/// the specialist tool").
#[tokio::test]
async fn front_door_toolbox_lists_specialist_tool() {
    // Specialist: publish enabled, mock backend irrelevant (the test
    // only exercises the published MCP surface, not the specialist's
    // own ReactAgent loop).
    let (spec_state, _ss, _sm) = build_state(
        specialist_toolbox(),
        Arc::new(MockBackend::with_response("specialist idle")),
        true,
    );
    let spec_addr = spawn(spec_state).await;
    let spec_mcp_url = format!("http://{spec_addr}/v1/mcp/serve");

    // Front-door: build its Toolbox by connecting an HttpMcpClient to
    // the specialist and registering every advertised tool. This
    // mirrors what McpSupervisor does at startup for any `mcp_servers`
    // row whose transport is "http".
    let http_client = Arc::new(
        HttpMcpClient::connect(HttpClientConfig::new(&spec_mcp_url))
            .await
            .expect("front-door connect to specialist"),
    );
    let remote_tools = http_client.list_tools().await.expect("list_tools");
    let fd_toolbox = Toolbox::from_server(http_client, remote_tools).expect("front-door toolbox");

    // Assertion #2: the specialist tool is visible to the front-door's
    // catalogue with the descriptor intact.
    let specs = fd_toolbox.to_specs();
    let tool = specs
        .iter()
        .find(|s| s.name == SPECIALIST_TOOL_NAME)
        .expect("specialist tool missing from front-door toolbox");
    assert_eq!(
        tool.description.as_deref(),
        Some("Summarise a webpage in three bullets.")
    );
    assert!(tool.parameters.get("properties").is_some());
}

/// End-to-end: POST a chat message to the front-door, the front-door's
/// scripted LLM emits a `tool_call` for the specialist's tool, the
/// front-door's ReactAgent dispatches that call across MCP to the
/// specialist, the specialist's `SummariserBackend` answers, the
/// front-door's LLM emits a final text reply, and the persisted
/// message history contains the specialist's reply in the tool
/// message + the final assistant text.
///
/// Covers the brief's assertion #3 ("End-to-end POST to front-door
/// triggers a remote call that completes successfully").
#[tokio::test]
async fn end_to_end_post_triggers_remote_call() {
    // 1. Specialist peer.
    let (spec_state, _ss, _sm) = build_state(
        specialist_toolbox(),
        Arc::new(MockBackend::with_response("specialist idle")),
        true,
    );
    let spec_addr = spawn(spec_state).await;
    let spec_mcp_url = format!("http://{spec_addr}/v1/mcp/serve");

    // 2. Connect HttpMcpClient and seed the front-door Toolbox.
    let http_client = Arc::new(
        HttpMcpClient::connect(HttpClientConfig::new(&spec_mcp_url))
            .await
            .expect("front-door connect to specialist"),
    );
    let remote_tools = http_client.list_tools().await.expect("list_tools");
    let fd_toolbox = Toolbox::from_server(http_client, remote_tools).expect("front-door toolbox");

    // 3. Front-door peer. Its backend is scripted to (a) call the
    //    specialist tool, then (b) answer with the user-facing text.
    let target_url = "https://example.com/news";
    let fd_backend: Arc<dyn LlmBackend> = Arc::new(MockBackend::with_script(vec![
        ScriptStep::tool_calls(vec![ToolCallSpec {
            id: "call_1".into(),
            name: SPECIALIST_TOOL_NAME.into(),
            arguments_json: json!({ "url": target_url }).to_string(),
        }]),
        ScriptStep::text("Here's the summary you asked for."),
    ]));
    let (fd_state, _fd_sessions, fd_messages) = build_state(fd_toolbox, fd_backend, false);
    let fd_addr = spawn(fd_state).await;

    // 4. Create a session + POST a message that should trip the tool.
    let http = reqwest::Client::new();
    let create = http
        .post(format!("http://{fd_addr}/v1/sessions"))
        .json(&json!({
            "user_id": "usr_demo",
            "tenant_id": "ten_demo",
            "model": "mock-model"
        }))
        .send()
        .await
        .expect("create session");
    assert_eq!(create.status().as_u16(), 201, "create session HTTP status");
    let session: JsonValue = create.json().await.expect("session json");
    let sid = session["id"].as_str().expect("session id").to_string();

    let send = http
        .post(format!("http://{fd_addr}/v1/sessions/{sid}/messages"))
        .json(&json!({ "content": format!("please summarise {target_url}") }))
        .send()
        .await
        .expect("send message");
    assert_eq!(send.status().as_u16(), 200, "send message HTTP status");
    let stream_body = send.bytes().await.expect("stream body");
    let stream_text = String::from_utf8_lossy(&stream_body);
    // The SSE stream should include the final assistant text from the
    // second scripted step — proof the front-door's ReAct loop ran to
    // completion *after* the remote tool call returned.
    assert!(
        stream_text.contains("Here's the summary you asked for."),
        "stream missing final assistant text:\n{stream_text}"
    );

    // 5. Let the finalize task land its persistence writes, then
    //    inspect the front-door's message history.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let stored = fd_messages.snapshot(&sid);
    // Expected sequence: user → assistant(tool_calls) → tool(result) → assistant(text).
    assert_eq!(stored.len(), 4, "stored = {stored:?}");
    assert_eq!(stored[0].role, xiaoguai_types::MessageRole::User);
    assert_eq!(stored[1].role, xiaoguai_types::MessageRole::Assistant);
    assert_eq!(stored[2].role, xiaoguai_types::MessageRole::Tool);
    assert_eq!(stored[3].role, xiaoguai_types::MessageRole::Assistant);

    // The tool message must carry the specialist's reply — that's the
    // proof the remote call round-tripped through MCP and the
    // specialist's `SummariserBackend::call_tool` actually ran. The
    // ReactAgent persists tool returns as `ContentBlock::ToolResult`
    // whose `output` is whatever the tool emitted; we serialize the
    // whole tool message back to JSON and grep for the marker.
    let tool_json = serde_json::to_string(&stored[2]).expect("serialize tool message");
    assert!(
        tool_json.contains(SPECIALIST_REPLY_PREFIX) && tool_json.contains(target_url),
        "tool message missing specialist reply: {tool_json}"
    );
}
