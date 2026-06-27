// Wire types mirrored from the xiaoguai serve.
//
// `AgentEvent` mirrors `crates/xiaoguai-agent`'s event union as serialised over
// SSE (see `crates/xiaoguai-api/src/sse.rs`): each frame's `data:` JSON carries
// a `type` discriminator equal to the SSE `event:` tag. We only model the
// fields the floater renders; extra fields are tolerated.

export type AgentEvent =
  | { type: 'text_delta'; delta: string }
  | { type: 'tool_call_started'; id: string; name: string; arguments: unknown }
  | {
      type: 'tool_call_finished';
      id: string;
      name: string;
      ok: boolean;
      error?: string | null;
      output_text?: string | null;
    }
  | { type: 'iteration_completed'; iteration: number }
  | { type: 'done'; stop_reason: 'completed' | 'max_iterations' | 'cancelled' }
  | { type: 'error'; message: string }
  // HotL governance frames — surfaced as a status line in this minimal client.
  | { type: 'hotl_pending'; tool: string; scope: string }
  | { type: 'hotl_resolved'; verdict: 'allow' | 'deny' | 'timeout' };

/**
 * A frame forwarded by the Rust side over the `chat://event` channel. Mirrors
 * the `ChatFrame` enum in `src-tauri/src/serve_client.rs` (serde
 * `tag = "kind"`, snake_case). Transport errors are not frames — the
 * `invoke('send_message')` promise rejects instead (handled in main.ts).
 */
export type ChatFrame =
  | { kind: 'agent'; data: AgentEvent }
  | { kind: 'stream_end' };
