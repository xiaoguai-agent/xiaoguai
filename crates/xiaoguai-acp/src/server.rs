//! The ACP dispatch loop.
//!
//! Owns the session table + per-session cancellation, frames messages via
//! [`transport`](crate::transport), and routes the four P2 methods to the
//! [`AcpDelegate`]. Knows nothing about the agent itself.
//!
//! Concurrency: a `session/prompt` turn is run on a spawned task while the read
//! loop keeps reading, so a `session/cancel` notification arriving mid-turn is
//! processed immediately (rather than blocking behind the in-flight turn). All
//! writes funnel through the cloneable, internally-synchronized [`LineWriter`].

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::acp::{
    AgentCapabilities, CancelNotification, ContentBlock, Implementation, InitializeRequest,
    InitializeResponse, NewSessionResponse, PromptRequest, PromptResponse, ProtocolVersion,
    SessionNotification,
};
use crate::delegate::{AcpDelegate, UpdateSink};
use crate::jsonrpc::{self, codes, Incoming};
use crate::transport::{LineReader, LineWriter};
use crate::{methods, PROTOCOL_VERSION};

/// `session/update` is the method the agent emits turn progress under.
const SESSION_UPDATE: &str = "session/update";

/// Shared, cloneable handle to the per-session cancellation tokens.
type Cancels = Arc<Mutex<HashMap<String, CancellationToken>>>;

/// Serve the ACP protocol over `reader`/`writer` until EOF, dispatching prompt
/// turns to `delegate`.
///
/// # Errors
/// Returns an I/O error only on an unrecoverable transport failure; malformed
/// or unsupported messages are answered with JSON-RPC errors and the loop
/// continues.
pub async fn serve<R, W>(
    delegate: Arc<dyn AcpDelegate>,
    reader: R,
    writer: W,
) -> std::io::Result<()>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let mut reader = LineReader::new(reader);
    let writer = LineWriter::new(writer);
    let sessions: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let cancels: Cancels = Arc::new(Mutex::new(HashMap::new()));
    let session_counter = Arc::new(AtomicU64::new(0));
    // In-flight prompt turns; drained on shutdown so a turn isn't abandoned
    // mid-write when the editor disconnects.
    let mut turns: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

    while let Some(line) = reader.next_message().await? {
        let incoming: Incoming = match serde_json::from_str(&line) {
            Ok(i) => i,
            Err(e) => {
                let id = recover_id(&line);
                writer
                    .write_message(&jsonrpc::error(
                        id,
                        codes::PARSE_ERROR,
                        format!("invalid JSON-RPC message: {e}"),
                    ))
                    .await?;
                continue;
            }
        };

        match incoming.method.as_str() {
            methods::INITIALIZE => {
                let Some(id) = incoming.id.clone() else {
                    continue;
                };
                writer
                    .write_message(&handle_initialize(&incoming, id))
                    .await?;
            }
            methods::SESSION_NEW => {
                let Some(id) = incoming.id.clone() else {
                    continue;
                };
                let session_id = format!("acp-{}", session_counter.fetch_add(1, Ordering::Relaxed));
                sessions.lock().await.insert(session_id.clone());
                let result = serde_json::to_value(NewSessionResponse::new(session_id))
                    .unwrap_or(Value::Null);
                writer.write_message(&jsonrpc::success(id, result)).await?;
            }
            methods::SESSION_PROMPT => {
                let Some(id) = incoming.id.clone() else {
                    continue;
                };
                spawn_prompt_turn(
                    &delegate,
                    &writer,
                    &sessions,
                    &cancels,
                    &mut turns,
                    id,
                    incoming.params,
                )
                .await?;
            }
            methods::SESSION_CANCEL => {
                handle_cancel(&cancels, &incoming.params).await;
            }
            other => {
                if let Some(id) = incoming.id.clone() {
                    writer
                        .write_message(&jsonrpc::error(
                            id,
                            codes::METHOD_NOT_FOUND,
                            format!(
                                "method `{other}` is not supported; this agent handles \
                                 initialize, session/new, session/prompt, session/cancel"
                            ),
                        ))
                        .await?;
                } else {
                    tracing::debug!(method = %other, "ignoring unsupported notification");
                }
            }
        }
    }

    // EOF: the editor disconnected. Cancel any in-flight turns so they unwind
    // promptly, then drain them so their final writes complete before we return.
    for token in cancels.lock().await.values() {
        token.cancel();
    }
    while turns.join_next().await.is_some() {}
    Ok(())
}

/// Build the `initialize` response, echoing the client's protocol version
/// (capped at ours) and advertising our identity + default capabilities.
fn handle_initialize(incoming: &Incoming, id: Value) -> Value {
    let version = serde_json::from_value::<InitializeRequest>(incoming.params.clone())
        .map(|req| req.protocol_version)
        .unwrap_or(ProtocolVersion::V1);
    let _ = PROTOCOL_VERSION; // documented as our latest; v1 today
    let response = InitializeResponse::new(version)
        .agent_capabilities(AgentCapabilities::new())
        .agent_info(Implementation::new("xiaoguai", env!("CARGO_PKG_VERSION")));
    let result = serde_json::to_value(response).unwrap_or(Value::Null);
    jsonrpc::success(id, result)
}

/// Validate a `session/prompt`, then run the turn on a spawned task so the read
/// loop stays responsive to `session/cancel`.
async fn spawn_prompt_turn<W>(
    delegate: &Arc<dyn AcpDelegate>,
    writer: &LineWriter<W>,
    sessions: &Arc<Mutex<HashSet<String>>>,
    cancels: &Cancels,
    turns: &mut tokio::task::JoinSet<()>,
    id: Value,
    params: Value,
) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin + Send + 'static,
{
    let req: PromptRequest = match serde_json::from_value(params) {
        Ok(r) => r,
        Err(e) => {
            return writer
                .write_message(&jsonrpc::error(
                    id,
                    codes::INVALID_PARAMS,
                    format!("invalid session/prompt params: {e}"),
                ))
                .await;
        }
    };
    let session_id = req.session_id.0.to_string();
    if !sessions.lock().await.contains(&session_id) {
        return writer
            .write_message(&jsonrpc::error(
                id,
                codes::INVALID_PARAMS,
                format!("unknown session `{session_id}`; call session/new first"),
            ))
            .await;
    }
    let text = extract_text(&req.prompt);

    let cancel = CancellationToken::new();
    cancels
        .lock()
        .await
        .insert(session_id.clone(), cancel.clone());

    let delegate = Arc::clone(delegate);
    let writer = writer.clone();
    let cancels = Arc::clone(cancels);
    turns.spawn(async move {
        run_prompt_turn(delegate, writer, id, session_id.clone(), text, cancel).await;
        cancels.lock().await.remove(&session_id);
    });
    Ok(())
}

/// Drive one turn: stream the delegate's updates as `session/update`
/// notifications, then resolve the original request with the stop reason.
async fn run_prompt_turn<W>(
    delegate: Arc<dyn AcpDelegate>,
    writer: LineWriter<W>,
    id: Value,
    session_id: String,
    text: String,
    cancel: CancellationToken,
) where
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (sink, mut rx) = UpdateSink::channel();
    let turn = delegate.prompt(&session_id, text, sink, cancel);
    tokio::pin!(turn);

    let stop = loop {
        tokio::select! {
            biased;
            Some(update) = rx.recv() => emit_update(&writer, &session_id, update).await,
            stop = &mut turn => break stop,
        }
    };
    // The delegate has returned and dropped its sink; flush any buffered
    // updates before the response so ordering holds.
    while let Ok(update) = rx.try_recv() {
        emit_update(&writer, &session_id, update).await;
    }

    let result = serde_json::to_value(PromptResponse::new(stop)).unwrap_or(Value::Null);
    if let Err(e) = writer.write_message(&jsonrpc::success(id, result)).await {
        tracing::warn!(error = %e, "failed to write prompt response");
    }
}

/// Emit one `session/update` notification (best-effort; a write failure ends
/// the connection on the next read, so it is only logged here).
async fn emit_update<W>(writer: &LineWriter<W>, session_id: &str, update: crate::acp::SessionUpdate)
where
    W: AsyncWrite + Unpin,
{
    let note = SessionNotification::new(session_id.to_string(), update);
    match serde_json::to_value(&note) {
        Ok(params) => {
            if let Err(e) = writer
                .write_message(&jsonrpc::notification(SESSION_UPDATE, params))
                .await
            {
                tracing::warn!(error = %e, "failed to write session/update");
            }
        }
        Err(e) => tracing::warn!(error = %e, "failed to serialize session/update"),
    }
}

/// Fire the cancellation token for the named session, if a turn is in flight.
async fn handle_cancel(cancels: &Cancels, params: &Value) {
    let Ok(note) = serde_json::from_value::<CancelNotification>(params.clone()) else {
        tracing::debug!("ignoring malformed session/cancel");
        return;
    };
    let session_id = note.session_id.0.to_string();
    if let Some(token) = cancels.lock().await.get(&session_id) {
        token.cancel();
    }
}

/// Concatenate the text content blocks of a prompt; non-text blocks are skipped
/// in P2 (no image/audio/resource support advertised).
fn extract_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Best-effort recovery of a request `id` from a line that failed to parse as
/// `Incoming`, so an error response can still target it.
fn recover_id(line: &str) -> Value {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|v| v.get("id").cloned())
        .unwrap_or(Value::Null)
}
