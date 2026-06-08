/**
 * /loop chat slash-command parser (L2b — DEC-039 / LLD-LOOP-001).
 *
 * The chat composer intercepts a draft that starts with `/loop` and routes it
 * here instead of sending it to the agent. Parsing is pure so it can be unit
 * tested in isolation; the side-effecting bits (createLoop / listLoops /
 * cancelLoop calls + bubble rendering) live in ChatPage.
 *
 * Grammar:
 *   /loop                  → help (bare command)
 *   /loop help             → help
 *   /loop status           → list this session's loops
 *   /loop cancel           → cancel the session's live loop
 *   /loop cancel <id>      → cancel a specific loop id
 *   /loop <prompt text...> → arm a recurring loop with that prompt
 *
 * `status`, `cancel` and `help` are reserved first words; any other non-empty
 * remainder is treated as the loop prompt.
 */

import type { LoopResponse, LoopStatus } from '@xiaoguai/shared';

/** A parsed `/loop` command, or `none` when the text is not a /loop command. */
export type LoopCommand =
  | { kind: 'none' }
  | { kind: 'help' }
  | { kind: 'status' }
  | { kind: 'cancel'; id?: string }
  | { kind: 'create'; prompt: string };

const COMMAND = '/loop';

/**
 * Parse a raw composer draft. Returns `{ kind: 'none' }` for anything that is
 * not a `/loop` command (the caller then sends it to the agent as normal). The
 * match is on `/loop` as a whole word — `/looplike …` is NOT a command.
 */
export function parseLoopCommand(text: string): LoopCommand {
  const trimmed = text.trim();
  if (trimmed !== COMMAND && !trimmed.startsWith(`${COMMAND} `)) {
    return { kind: 'none' };
  }
  const rest = trimmed.slice(COMMAND.length).trim();
  if (rest === '') return { kind: 'help' };

  const parts = rest.split(/\s+/);
  const sub = parts[0]!.toLowerCase();
  if (sub === 'help') return { kind: 'help' };
  if (sub === 'status') return { kind: 'status' };
  if (sub === 'cancel') {
    const id = parts[1];
    return id ? { kind: 'cancel', id } : { kind: 'cancel' };
  }
  // Anything else is a free-form prompt (preserve the original casing/spacing).
  return { kind: 'create', prompt: rest };
}

/** Statuses that mean a loop will no longer tick. `active` / `paused` are live. */
export const TERMINAL_LOOP_STATUSES: ReadonlySet<LoopStatus> = new Set<LoopStatus>([
  'budget_exhausted',
  'done',
  'cancelled',
  'failed',
]);

/** True while a loop can still tick (i.e. is `active` or `paused`). */
export function isLoopLive(loop: LoopResponse): boolean {
  return !TERMINAL_LOOP_STATUSES.has(loop.status);
}

/** Short, human-friendly loop id (first 8 chars of the UUID). */
export function shortLoopId(id: string): string {
  return id.slice(0, 8);
}
