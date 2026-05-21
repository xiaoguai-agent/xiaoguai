//! `ReAct` loop: think → `tool_call(s)` in parallel → observe → loop until done.
//!
//! Architecture:
//!
//! ```text
//!   ┌─────────┐    ChatRequest (msgs + tools)    ┌──────────┐
//!   │ History │ ───────────────────────────────► │   LLM    │
//!   └────┬────┘                                  └────┬─────┘
//!        │       text deltas + tool_calls             │
//!        │ ◄──────────────────────────────────────────┘
//!        │
//!        │  for each tool_call: client.call_tool(args)
//!        ▼
//!   ┌─────────────┐
//!   │  Toolbox    │  ── parallel via try_join_all ──► append Tool messages
//!   │ (MCP fanout)│
//!   └─────────────┘
//! ```
//!
//! Cancellation: a `CancellationToken` is checked between iterations and
//! before each tool dispatch fanout. In-flight LLM streams complete to a
//! natural boundary first to keep the event sequence consistent.
//!
//! Events are emitted to a `tokio::sync::mpsc` channel returned to the
//! caller as a `ReceiverStream`. The loop tolerates a disconnected receiver
//! (the caller dropped the stream); we stop sending but keep running until
//! a stop reason is hit, so audit / persistence layers downstream from the
//! agent still see the final state.

use std::sync::Arc;

use futures::StreamExt;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use xiaoguai_llm::{
    ChatRequest, LlmBackend, LlmError, Message, ToolCallSpec, ToolChoice, ToolSpec,
};
use xiaoguai_mcp::McpError;

use crate::event::{AgentEvent, StopReason};
use crate::history::slide;
use crate::toolbox::Toolbox;

#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// Hard cap on think→act→observe cycles. Reaching it ends the loop with
    /// `StopReason::MaxIterations`.
    pub max_iterations: u32,
    /// Slide-window size for non-system messages. `0` disables trimming.
    pub history_window: usize,
    /// Temperature passed through to every request. `None` = backend default.
    pub temperature: Option<f32>,
    pub model: String,
}

impl AgentConfig {
    #[must_use]
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            max_iterations: 8,
            history_window: 32,
            temperature: Some(0.2),
            model: model.into(),
        }
    }
}

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("llm backend error: {0}")]
    Llm(#[from] LlmError),
    #[error("agent loop failed: {0}")]
    Other(String),
}

/// The completed run's terminal state. `events` is the in-order record of
/// what was emitted to the stream; `messages` is the final conversation
/// after windowing (suitable for persisting).
#[derive(Debug, Clone)]
pub struct AgentOutcome {
    pub stop_reason: StopReason,
    pub messages: Vec<Message>,
    pub iterations: u32,
}

/// Driver for one ReAct conversation. Re-usable across runs as long as the
/// model + backend stay the same.
pub struct ReactAgent {
    backend: Arc<dyn LlmBackend>,
    toolbox: Arc<Toolbox>,
    config: AgentConfig,
}

impl ReactAgent {
    #[must_use]
    pub fn new(backend: Arc<dyn LlmBackend>, toolbox: Toolbox, config: AgentConfig) -> Self {
        Self {
            backend,
            toolbox: Arc::new(toolbox),
            config,
        }
    }

    /// Run to completion, draining the event stream into a vector. Useful
    /// for tests and synchronous callers that don't need streaming.
    pub async fn run_to_completion(
        &self,
        initial: Vec<Message>,
        cancel: CancellationToken,
    ) -> Result<(AgentOutcome, Vec<AgentEvent>), AgentError> {
        let (outcome_handle, mut stream) = self.run_stream(initial, cancel);
        let mut collected = Vec::new();
        while let Some(ev) = stream.next().await {
            collected.push(ev);
        }
        let outcome = outcome_handle
            .await
            .map_err(|e| AgentError::Other(format!("agent task join error: {e}")))??;
        Ok((outcome, collected))
    }

    /// Launch the loop in the background and return `(join_handle, stream)`.
    /// The stream completes when the loop terminates (regardless of stop
    /// reason); the join handle yields the structured outcome.
    pub fn run_stream(
        &self,
        initial: Vec<Message>,
        cancel: CancellationToken,
    ) -> (
        tokio::task::JoinHandle<Result<AgentOutcome, AgentError>>,
        ReceiverStream<AgentEvent>,
    ) {
        let (tx, rx) = mpsc::channel::<AgentEvent>(64);
        let backend = self.backend.clone();
        let toolbox = self.toolbox.clone();
        let config = self.config.clone();

        let handle =
            tokio::spawn(
                async move { run_inner(backend, toolbox, config, initial, cancel, tx).await },
            );
        (handle, ReceiverStream::new(rx))
    }
}

/// Outcome of streaming one LLM call.
struct ModelTurn {
    text: String,
    tool_calls: Vec<ToolCallSpec>,
}

async fn collect_model_turn(
    backend: &Arc<dyn LlmBackend>,
    req: ChatRequest,
    tx: &mpsc::Sender<AgentEvent>,
) -> Result<ModelTurn, AgentError> {
    let mut stream = backend.chat_stream(req).await.map_err(|e| {
        let msg = e.to_string();
        let tx2 = tx.clone();
        tokio::spawn(async move {
            let _ = tx2.send(AgentEvent::Error { message: msg }).await;
        });
        AgentError::Llm(e)
    })?;

    let mut text = String::new();
    let mut tool_calls: Vec<ToolCallSpec> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                emit(
                    tx,
                    AgentEvent::Error {
                        message: e.to_string(),
                    },
                )
                .await;
                return Err(AgentError::Llm(e));
            }
        };
        if !chunk.delta.is_empty() {
            text.push_str(&chunk.delta);
            emit(tx, AgentEvent::TextDelta { delta: chunk.delta }).await;
        }
        if chunk.done {
            tool_calls = chunk.tool_calls;
        }
    }
    Ok(ModelTurn { text, tool_calls })
}

fn assistant_message_from_turn(turn: &ModelTurn) -> Message {
    if turn.tool_calls.is_empty() {
        Message::assistant(turn.text.clone())
    } else if turn.text.is_empty() {
        Message::assistant_tool_calls(turn.tool_calls.clone())
    } else {
        Message {
            role: xiaoguai_llm::Role::Assistant,
            content: turn.text.clone(),
            tool_calls: turn.tool_calls.clone(),
            tool_call_id: None,
        }
    }
}

fn build_request(
    config: &AgentConfig,
    messages: &[Message],
    tool_specs: &[ToolSpec],
) -> ChatRequest {
    let mut req = ChatRequest::new(config.model.clone(), messages.to_vec());
    req.temperature = config.temperature;
    if !tool_specs.is_empty() {
        req.tools = tool_specs.to_vec();
        req.tool_choice = ToolChoice::Auto;
    }
    req
}

async fn finish_with(
    tx: &mpsc::Sender<AgentEvent>,
    stop: StopReason,
    messages: Vec<Message>,
    iterations: u32,
) -> AgentOutcome {
    emit(
        tx,
        AgentEvent::Done {
            stop_reason: stop.clone(),
        },
    )
    .await;
    AgentOutcome {
        stop_reason: stop,
        messages,
        iterations,
    }
}

async fn run_inner(
    backend: Arc<dyn LlmBackend>,
    toolbox: Arc<Toolbox>,
    config: AgentConfig,
    initial: Vec<Message>,
    cancel: CancellationToken,
    tx: mpsc::Sender<AgentEvent>,
) -> Result<AgentOutcome, AgentError> {
    let mut messages = initial;
    let tool_specs = toolbox.to_specs();
    let mut iteration: u32 = 0;

    loop {
        if cancel.is_cancelled() {
            return Ok(finish_with(&tx, StopReason::Cancelled, messages, iteration).await);
        }
        if iteration >= config.max_iterations {
            return Ok(finish_with(&tx, StopReason::MaxIterations, messages, iteration).await);
        }

        messages = slide(messages, config.history_window);
        let req = build_request(&config, &messages, &tool_specs);
        let turn = collect_model_turn(&backend, req, &tx).await?;
        messages.push(assistant_message_from_turn(&turn));

        if turn.tool_calls.is_empty() {
            emit(&tx, AgentEvent::IterationCompleted { iteration }).await;
            return Ok(finish_with(&tx, StopReason::Completed, messages, iteration + 1).await);
        }

        if cancel.is_cancelled() {
            return Ok(finish_with(&tx, StopReason::Cancelled, messages, iteration + 1).await);
        }

        let results = dispatch_tools(&toolbox, &turn.tool_calls, &tx).await;
        for (call, tr) in turn.tool_calls.iter().zip(results.into_iter()) {
            messages.push(tool_message_for(call, &tr));
        }

        emit(&tx, AgentEvent::IterationCompleted { iteration }).await;
        iteration += 1;
    }
}

/// Outcome of one tool dispatch — the payload we'll inject as a `Role::Tool`
/// message, plus the bookkeeping `AgentEvent::ToolCallFinished` needs.
struct ToolDispatchOutcome {
    ok: bool,
    output_text: String,
    error: Option<String>,
}

async fn dispatch_tools(
    toolbox: &Toolbox,
    calls: &[ToolCallSpec],
    tx: &mpsc::Sender<AgentEvent>,
) -> Vec<ToolDispatchOutcome> {
    // Pre-emit started events in the same order we'll dispatch.
    for call in calls {
        let args = parse_args(&call.arguments_json);
        emit(
            tx,
            AgentEvent::ToolCallStarted {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: args,
            },
        )
        .await;
    }

    // Build per-call futures. Each future captures its `ToolCallSpec` so the
    // result row stays addressable by id when we zip with the original list.
    let futs = calls.iter().map(|call| {
        let entry = toolbox.get(&call.name);
        let id = call.id.clone();
        let name = call.name.clone();
        let args_json = call.arguments_json.clone();
        async move {
            let outcome = match entry {
                None => ToolDispatchOutcome {
                    ok: false,
                    output_text: String::new(),
                    error: Some(format!("tool {name:?} not in toolbox")),
                },
                Some(entry) => {
                    let parsed = parse_args(&args_json);
                    match entry.client.call_tool(&name, parsed).await {
                        Ok(tr) if tr.is_error => ToolDispatchOutcome {
                            ok: false,
                            output_text: tr.text.clone(),
                            error: Some(tr.text),
                        },
                        Ok(tr) => ToolDispatchOutcome {
                            ok: true,
                            output_text: tr.text,
                            error: None,
                        },
                        Err(e) => map_mcp_err(&e),
                    }
                }
            };
            (id, name, outcome)
        }
    });

    let results = futures::future::join_all(futs).await;

    // Emit finished events + return outcomes in input order.
    let mut outcomes = Vec::with_capacity(results.len());
    for (id, name, outcome) in results {
        emit(
            tx,
            AgentEvent::ToolCallFinished {
                id,
                name,
                ok: outcome.ok,
                error: outcome.error.clone(),
                output_text: if outcome.output_text.is_empty() {
                    None
                } else {
                    Some(outcome.output_text.clone())
                },
            },
        )
        .await;
        outcomes.push(outcome);
    }
    outcomes
}

fn map_mcp_err(e: &McpError) -> ToolDispatchOutcome {
    ToolDispatchOutcome {
        ok: false,
        output_text: String::new(),
        error: Some(e.to_string()),
    }
}

fn parse_args(raw: &str) -> serde_json::Value {
    if raw.is_empty() {
        return serde_json::json!({});
    }
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_string()))
}

fn tool_message_for(call: &ToolCallSpec, outcome: &ToolDispatchOutcome) -> Message {
    let content = if outcome.ok {
        if outcome.output_text.is_empty() {
            "(no output)".to_string()
        } else {
            outcome.output_text.clone()
        }
    } else {
        // Render errors as a structured payload so the model can recover.
        serde_json::json!({
            "error": outcome.error.clone().unwrap_or_else(|| "tool failed".into()),
            "text": outcome.output_text,
        })
        .to_string()
    };
    Message::tool(call.id.clone(), content)
}

async fn emit(tx: &mpsc::Sender<AgentEvent>, ev: AgentEvent) {
    if let Err(err) = tx.send(ev).await {
        // The caller dropped the receiver; we keep running but stop sending.
        debug!(?err, "agent event receiver dropped");
    }
}
