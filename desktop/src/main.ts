// Floater UI controller. Owns the conversation state and wires DOM events to
// the Rust bridge (chat.ts). Kept small: rendering lives in view.ts, the
// network/Tauri bridge in chat.ts, wire types in types.ts.

import { listen } from '@tauri-apps/api/event';

import { hideWindow, onChatFrame, resetSession, sendMessage } from './chat';
import type { AgentEvent } from './types';
import {
  addBubble,
  addToolLine,
  appendText,
  autoGrow,
  el,
  setStatus,
} from './view';

const FOCUS_INPUT_EVENT = 'floater://focus-input';

const messagesEl = el<HTMLDivElement>('messages');
const inputEl = el<HTMLTextAreaElement>('input');
const statusEl = el<HTMLSpanElement>('status');

/** The assistant bubble currently being streamed into, if any. */
let streamingBubble: HTMLDivElement | null = null;
/** Guards against overlapping sends (the serve rejects a 2nd in-flight turn). */
let sending = false;

/** Apply one streamed `AgentEvent` to the DOM. */
function applyAgentEvent(ev: AgentEvent): void {
  switch (ev.type) {
    case 'text_delta': {
      if (!streamingBubble) streamingBubble = addBubble(messagesEl, 'assistant');
      appendText(streamingBubble, ev.delta);
      break;
    }
    case 'tool_call_started': {
      addToolLine(messagesEl, `→ ${ev.name}(${JSON.stringify(ev.arguments)})`, false);
      // A fresh assistant bubble follows a tool call.
      streamingBubble = addBubble(messagesEl, 'assistant');
      break;
    }
    case 'tool_call_finished': {
      const line = ev.ok
        ? `← ${ev.name}: ${ev.output_text ?? '(无输出)'}`
        : `✗ ${ev.name}: ${ev.error ?? '失败'}`;
      addToolLine(messagesEl, line, !ev.ok);
      break;
    }
    case 'iteration_completed':
      // Nothing to render; the next text_delta reuses/creates the bubble.
      break;
    case 'done':
      setStatus(statusEl, `完成 · ${ev.stop_reason}`);
      finishTurn();
      break;
    case 'error':
      setStatus(statusEl, `出错: ${ev.message}`);
      addBubble(messagesEl, 'system', `⚠️ ${ev.message}`);
      finishTurn();
      break;
    case 'hotl_pending':
      setStatus(statusEl, `等待批准: ${ev.tool}`);
      break;
    case 'hotl_resolved':
      setStatus(statusEl, `审批: ${ev.verdict}`);
      break;
    default:
      // Unknown event type — ignore (forward-compat with new serve events).
      break;
  }
}

/** Settle UI state at the end of a turn (terminal frame or stream end). */
function finishTurn(): void {
  sending = false;
  streamingBubble = null;
  inputEl.disabled = false;
  inputEl.focus();
}

/** Send the current input, if any and not already sending. */
async function submit(): Promise<void> {
  const text = inputEl.value.trim();
  if (!text || sending) return;

  sending = true;
  inputEl.value = '';
  autoGrow(inputEl);
  inputEl.disabled = true;
  streamingBubble = null;

  addBubble(messagesEl, 'user', text);
  setStatus(statusEl, '思考中…');

  try {
    await sendMessage(text);
    // `done`/`error`/`stream_end` settle the UI; if none arrived (e.g. the
    // stream closed silently), finishTurn() is also called on stream_end.
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    setStatus(statusEl, '连接失败');
    addBubble(messagesEl, 'system', `⚠️ ${message}`);
    finishTurn();
  }
}

/** Keyboard handling on the composer: Enter sends, Shift+Enter newline, Esc hides. */
function onInputKeydown(event: KeyboardEvent): void {
  if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
    event.preventDefault();
    void submit();
  } else if (event.key === 'Escape') {
    event.preventDefault();
    void hideWindow();
  }
}

/** Wire all listeners and focus the input. */
async function bootstrap(): Promise<void> {
  inputEl.addEventListener('keydown', onInputKeydown);
  inputEl.addEventListener('input', () => autoGrow(inputEl));

  // Stream frames from Rust.
  await onChatFrame((frame) => {
    switch (frame.kind) {
      case 'agent':
        applyAgentEvent(frame.data);
        break;
      case 'stream_end':
        // Re-enable the composer even if no terminal `done` arrived.
        if (sending) finishTurn();
        break;
    }
  });

  // When the window is summoned (global shortcut), Rust emits this so we focus
  // the composer for an instant type-and-go experience.
  await listen(FOCUS_INPUT_EVENT, () => {
    inputEl.focus();
    inputEl.select();
  });

  inputEl.focus();
}

// Esc also resets the conversation context on next open is intentionally NOT
// done — the window keeps history while alive. `resetSession` is exported for a
// future "new chat" affordance.
void resetSession; // referenced to satisfy noUnusedLocals without behaviour.

void bootstrap();
