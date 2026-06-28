// Bridge between the floating UI and the Rust backend.
//
// All HTTP to the serve happens in Rust (to bypass webview CORS and stream
// SSE); this module just invokes the Tauri commands and subscribes to the
// `chat://event` channel the Rust side emits per parsed frame.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

import type { ChatFrame } from './types';

const CHAT_EVENT = 'chat://event';

/** Lazily-created session id, reused for the lifetime of the window. */
let sessionId: string | null = null;

/** Reset the conversation — the next send creates a fresh session. */
export function resetSession(): void {
  sessionId = null;
}

/** Ensure a session exists, creating one on first use. */
async function ensureSession(): Promise<string> {
  if (sessionId) return sessionId;
  // Rust command `create_session` -> POST /v1/sessions -> returns the id.
  sessionId = await invoke<string>('create_session');
  return sessionId;
}

/**
 * Send a user message. The reply streams back via {@link onChatFrame}; this
 * promise resolves once the Rust side finishes draining the SSE stream.
 * Throws (with the Rust error string) if the session create or request fails.
 */
export async function sendMessage(content: string): Promise<void> {
  const id = await ensureSession();
  // Rust command `send_message` -> POST /v1/sessions/{id}/messages (SSE);
  // frames are emitted on CHAT_EVENT as they arrive.
  await invoke('send_message', { sessionId: id, content });
}

/**
 * Subscribe to streamed chat frames. Returns an unlisten function. Register
 * this once at startup, before sending anything.
 */
export function onChatFrame(handler: (frame: ChatFrame) => void): Promise<UnlistenFn> {
  return listen<ChatFrame>(CHAT_EVENT, (event) => handler(event.payload));
}

/** Ask the Rust side to hide the window (used on Esc). */
export function hideWindow(): Promise<void> {
  return invoke('hide_window');
}
