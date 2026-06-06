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

use crate::event::{AgentEvent, HotlResolution as EventResolution, StopReason};
use crate::history::{compact, should_compact, slide, CompactionConfig, CompactionOutcome};
use crate::hotl_gate::{HotlGateVerdict, HotlResolution, HotlTicketError, SharedHotlGate};
use crate::toolbox::Toolbox;
use xiaoguai_llm::estimate_message_tokens;

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
    /// Tier-2 prereq: optional HOTL budget gate consulted before every
    /// tool dispatch. `None` (the default) preserves legacy behaviour —
    /// all tools execute unconditionally. When `Some`, the loop calls
    /// [`crate::hotl_gate::HotlGate::check`] with
    /// `scope = format!("tool_call.{tool_name}")` per call; a `Deny`
    /// verdict suppresses the dispatch and reports the reason back to
    /// the model as a synthetic tool failure.
    ///
    /// `Option<None>` (no enforcer wired) ≠ enforcer infra error: the
    /// latter is folded into `Deny` by the adapter living in
    /// `xiaoguai-core::hotl_bridge::EnforcerGate`.
    pub hotl_gate: Option<SharedHotlGate>,
    /// v0.5.4.1: LLM-summarisation compaction. When `Some`, the loop
    /// switches from pure `slide` trimming to `compact` whenever the
    /// estimated token count crosses
    /// [`CompactionConfig::trigger_threshold`]. Below the threshold the
    /// `slide` path still runs to enforce the `history_window` cap. When
    /// `None`, behaviour is identical to pre-compaction (legacy).
    pub compaction: Option<CompactionConfig>,
}

impl AgentConfig {
    #[must_use]
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            max_iterations: 8,
            history_window: 32,
            temperature: Some(0.2),
            model: model.into(),
            hotl_gate: None,
            compaction: None,
        }
    }

    /// Builder-style attach for the HOTL gate. Chains nicely with
    /// `AgentConfig::new(model)`.
    #[must_use]
    pub fn with_hotl_gate(mut self, gate: SharedHotlGate) -> Self {
        self.hotl_gate = Some(gate);
        self
    }

    /// Builder-style enable for LLM-summarisation compaction.
    #[must_use]
    pub fn with_compaction(mut self, cfg: CompactionConfig) -> Self {
        self.compaction = Some(cfg);
        self
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
    ///
    /// # Errors
    /// Returns an error if the agent task panics or the LLM backend fails.
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
    #[must_use]
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

        // History management:
        //   * Legacy path (compaction = None): `slide` with the configured
        //     fixed window.
        //   * Compaction path: when token estimate exceeds the trigger
        //     threshold, call `compact` (LLM summary + slide fallback).
        //     Below threshold, still run `slide` so the window bound is
        //     respected when the conversation is short.
        if let Some(cfg) = config.compaction {
            if should_compact(&messages, cfg) {
                let before_tokens = estimate_message_tokens(&messages);
                let (next, outcome) = compact(messages, backend.as_ref(), cfg).await;
                messages = next;
                let after_tokens = estimate_message_tokens(&messages);
                match outcome {
                    CompactionOutcome::Compacted => {
                        if let Some(c) = xiaoguai_observability::compaction_triggered_total() {
                            c.with_label_values(&["threshold"]).inc();
                        }
                        if let Some(h) = xiaoguai_observability::compaction_token_savings() {
                            h.observe(before_tokens.saturating_sub(after_tokens) as f64);
                        }
                        tracing::info!(
                            target: "xiaoguai_agent::react",
                            kept = messages.len(),
                            before_tokens,
                            after_tokens,
                            "history compacted (LLM summary)"
                        );
                    }
                    CompactionOutcome::FellBack => {
                        if let Some(c) = xiaoguai_observability::compaction_triggered_total() {
                            c.with_label_values(&["threshold"]).inc();
                        }
                        if let Some(c) = xiaoguai_observability::compaction_fallback_total() {
                            c.with_label_values(&["backend_error"]).inc();
                        }
                        tracing::warn!(
                            target: "xiaoguai_agent::react",
                            kept = messages.len(),
                            "history compaction fell back to slide"
                        );
                    }
                    CompactionOutcome::NoOp => {}
                }
            } else {
                messages = slide(messages, config.history_window);
            }
        } else {
            messages = slide(messages, config.history_window);
        }
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

        let results = dispatch_tools(
            &toolbox,
            &turn.tool_calls,
            config.hotl_gate.as_ref(),
            &tx,
            &cancel,
        )
        .await;
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
    hotl_gate: Option<&SharedHotlGate>,
    tx: &mpsc::Sender<AgentEvent>,
    cancel: &CancellationToken,
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
    //
    // Tier-2 prereq: each future consults the HOTL gate **before** invoking
    // the MCP client. The check happens inside the per-call future so
    // parallel dispatch still produces one budget event per tool call (NOT
    // one for the batch). On `Deny`, we short-circuit to a failed
    // `ToolDispatchOutcome` carrying the reason — the LLM observes the
    // denial via the `Role::Tool` message and can adapt.
    let total_calls = calls.len();
    let futs = calls.iter().enumerate().map(|(idx, call)| {
        let entry = toolbox.get(&call.name);
        let id = call.id.clone();
        let name = call.name.clone();
        let args_json = call.arguments_json.clone();
        let gate = hotl_gate.cloned();
        let tx_inner = tx.clone();
        let cancel_inner = cancel.clone();
        // Bound the per-turn tool-call fan-out: a misbehaving model can emit
        // hundreds of calls, and `join_all` would spawn one concurrent future
        // (each holding a HotL ticket + MCP round-trip) per call. Execute the
        // first MAX and reject the rest with an aligned outcome that tells the
        // model to re-issue them — preserving 1:1 order so the caller's zip
        // stays correct.
        let over_limit = idx >= MAX_TOOL_CALLS_PER_TURN;
        async move {
            if over_limit {
                let outcome = ToolDispatchOutcome {
                    ok: false,
                    output_text: String::new(),
                    error: Some(format!(
                        "rejected: this turn requested {total_calls} tool calls; only the \
                         first {MAX_TOOL_CALLS_PER_TURN} run per turn. Re-issue the remaining \
                         calls in a follow-up turn."
                    )),
                };
                return (id, name, outcome);
            }
            // 1. HOTL pre-check. No gate → bypass.
            if let Some(gate) = gate.as_ref() {
                let scope = format!("tool_call.{name}");
                // Sprint-13 S13-6: hand the parsed args to the gate so
                // `SuspendingHotlGate` can run them through the
                // `RedactionRules`. The default trait impl backfills
                // `args_redacted` with the verbatim args for adapters
                // that don't override.
                let args_for_gate = parse_args(&args_json);
                let verdict = gate.check_with_args(&scope, 1.0, &args_for_gate).await;
                match verdict {
                    HotlGateVerdict::Allow => {}
                    HotlGateVerdict::Deny(reason) => {
                        let outcome = ToolDispatchOutcome {
                            ok: false,
                            output_text: String::new(),
                            error: Some(format!("HOTL gate denied tool '{name}': {reason}")),
                        };
                        return (id, name, outcome);
                    }
                    HotlGateVerdict::Suspend {
                        escalation_id,
                        scope: suspend_scope,
                        ticket,
                        args_redacted,
                    } => {
                        // Sprint-12 (S12-5). Emit HotlPending so SSE clients
                        // render the operator banner, then block this branch
                        // of the parallel dispatch on the ticket. Other
                        // tool calls in the same turn keep running — the
                        // outer `join_all` collects them as they complete.
                        //
                        // expires_at: convert tokio's Instant → wall-clock
                        // chrono via the remaining duration. This is the
                        // best we can do without dragging a SystemTime
                        // through the gate; clients render it as a "due
                        // by" timestamp, ±tens of ms is irrelevant.
                        let now_instant = tokio::time::Instant::now();
                        let remaining = ticket.expires_at().saturating_duration_since(now_instant);
                        let expires_at_wall = chrono::Utc::now()
                            + chrono::Duration::from_std(remaining).unwrap_or_else(|_| {
                                chrono::Duration::seconds(0)
                            });
                        // Sprint-13 S13-6: `args_redacted` is computed by
                        // `SuspendingHotlGate` (via `RedactionRules`)
                        // and threaded through the `Suspend` verdict.
                        // The default `HotlGate` impl backfills it from
                        // the raw args, so test stubs continue to work
                        // unchanged.
                        emit(
                            &tx_inner,
                            AgentEvent::HotlPending {
                                escalation_id,
                                tool: name.clone(),
                                args_redacted,
                                scope: suspend_scope,
                                expires_at: expires_at_wall,
                            },
                        )
                        .await;

                        // Block this branch until the operator decides, the
                        // deadline passes, or the parent cancels.
                        match ticket.await_decision(&cancel_inner).await {
                            Ok(decision) => {
                                let event_verdict = match &decision.verdict {
                                    HotlResolution::Allow => EventResolution::Allow,
                                    HotlResolution::Deny(_) => EventResolution::Deny,
                                    HotlResolution::Timeout => EventResolution::Timeout,
                                };
                                emit(
                                    &tx_inner,
                                    AgentEvent::HotlResolved {
                                        escalation_id,
                                        verdict: event_verdict,
                                        decided_by: decision.decided_by.clone(),
                                        recorded_at: decision.recorded_at,
                                    },
                                )
                                .await;
                                match decision.verdict {
                                    HotlResolution::Allow => {
                                        // Fall through to the normal dispatch path.
                                    }
                                    HotlResolution::Deny(reason) => {
                                        let outcome = ToolDispatchOutcome {
                                            ok: false,
                                            output_text: String::new(),
                                            error: Some(format!(
                                                "HotL suspended → {reason}"
                                            )),
                                        };
                                        return (id, name, outcome);
                                    }
                                    HotlResolution::Timeout => {
                                        let outcome = ToolDispatchOutcome {
                                            ok: false,
                                            output_text: String::new(),
                                            error: Some(
                                                "HotL suspended → timeout: operator did not decide before expires_at".to_string()
                                            ),
                                        };
                                        return (id, name, outcome);
                                    }
                                }
                            }
                            Err(HotlTicketError::Cancelled) => {
                                // DEC-LLD-AGENT-004 + lld-agent §4.5: do NOT
                                // emit HotlResolved when the parent cancels;
                                // the outer `cancel.is_cancelled()` check at
                                // the next iteration boundary terminates the
                                // loop with Final(Cancelled). Synthesise a
                                // failed dispatch so the join_all completes
                                // cleanly and the iteration unwinds.
                                let outcome = ToolDispatchOutcome {
                                    ok: false,
                                    output_text: String::new(),
                                    error: Some(
                                        "HotL suspension cancelled by parent operation".to_string(),
                                    ),
                                };
                                return (id, name, outcome);
                            }
                            Err(HotlTicketError::ChannelDropped) => {
                                // Registry sender dropped without sending —
                                // misconfiguration / race. Degrade to a
                                // synthetic deny + log so the loop doesn't
                                // hang forever. Still emit HotlResolved
                                // (verdict=Deny) so SSE clients clear the
                                // pending banner.
                                tracing::error!(
                                    %escalation_id,
                                    tool = %name,
                                    "DecisionRegistry sender dropped without verdict — synthesising deny"
                                );
                                emit(
                                    &tx_inner,
                                    AgentEvent::HotlResolved {
                                        escalation_id,
                                        verdict: EventResolution::Deny,
                                        decided_by: None,
                                        recorded_at: chrono::Utc::now(),
                                    },
                                )
                                .await;
                                let outcome = ToolDispatchOutcome {
                                    ok: false,
                                    output_text: String::new(),
                                    error: Some(
                                        "HotL suspended → registry channel dropped".to_string(),
                                    ),
                                };
                                return (id, name, outcome);
                            }
                        }
                    }
                }
            }

            // 2. Dispatch the tool (existing behaviour).
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

/// Max tool calls executed in a single agent turn. Excess calls are rejected
/// with a message telling the model to re-issue them (see `dispatch_tools`).
const MAX_TOOL_CALLS_PER_TURN: usize = 32;

/// Max bytes of a single tool result fed back into the model context. A tool
/// that returns a whole large file / a broad `grep` / a huge `git status`
/// would otherwise blow the context budget in one shot. Generous (~24k tokens)
/// so normal reads pass untouched; oversized results are truncated with a
/// marker that tells the model to narrow its query.
const MAX_TOOL_RESULT_BYTES: usize = 96_000;

/// Truncate `s` to at most `max` bytes on a UTF-8 char boundary. Returns the
/// (possibly borrowed) text and whether truncation happened. Never panics on
/// multibyte input (cf. the round-3 `build_summary` byte-slice bug).
fn truncate_on_char_boundary(s: &str, max: usize) -> (&str, bool) {
    if s.len() <= max {
        return (s, false);
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    (&s[..end], true)
}

/// Cap a tool result for context feedback, appending a marker when truncated.
fn cap_tool_result(text: &str) -> String {
    let (head, truncated) = truncate_on_char_boundary(text, MAX_TOOL_RESULT_BYTES);
    if truncated {
        format!(
            "{head}\n… (tool result truncated to {MAX_TOOL_RESULT_BYTES} bytes of \
             {} — narrow the query or read a specific range)",
            text.len()
        )
    } else {
        head.to_string()
    }
}

fn tool_message_for(call: &ToolCallSpec, outcome: &ToolDispatchOutcome) -> Message {
    let content = if outcome.ok {
        if outcome.output_text.is_empty() {
            "(no output)".to_string()
        } else {
            cap_tool_result(&outcome.output_text)
        }
    } else {
        // Render errors as a structured payload so the model can recover.
        serde_json::json!({
            "error": outcome.error.clone().unwrap_or_else(|| "tool failed".into()),
            "text": cap_tool_result(&outcome.output_text),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_on_char_boundary_never_splits_a_codepoint() {
        // Short input passes through untouched.
        let (s, t) = truncate_on_char_boundary("hello", 96_000);
        assert_eq!(s, "hello");
        assert!(!t);
        // A multibyte string whose cap lands mid-codepoint must back off to a
        // boundary, never panic (cf. the round-3 build_summary byte-slice bug).
        let multi = "你好世界".repeat(10); // 3 bytes/char
        let (head, truncated) = truncate_on_char_boundary(&multi, 7);
        assert!(truncated);
        assert!(head.len() <= 7);
        assert!(multi.starts_with(head)); // valid prefix, valid UTF-8
    }

    #[test]
    fn cap_tool_result_truncates_oversized_with_marker() {
        let small = "ok";
        assert_eq!(cap_tool_result(small), "ok");

        let big = "x".repeat(MAX_TOOL_RESULT_BYTES + 5_000);
        let capped = cap_tool_result(&big);
        assert!(capped.len() < big.len());
        assert!(capped.contains("truncated"));
        assert!(capped.starts_with("xxxx"));
    }

    #[test]
    fn tool_message_for_caps_a_huge_successful_result() {
        let call = ToolCallSpec {
            id: "call_1".into(),
            name: "read_file".into(),
            arguments_json: "{}".into(),
        };
        let outcome = ToolDispatchOutcome {
            ok: true,
            output_text: "A".repeat(MAX_TOOL_RESULT_BYTES * 2),
            error: None,
        };
        let msg = tool_message_for(&call, &outcome);
        // The fed-back content is bounded well under the raw output size.
        assert!(msg.content.len() < MAX_TOOL_RESULT_BYTES + 500);
        assert!(msg.content.contains("truncated"));
    }
}
