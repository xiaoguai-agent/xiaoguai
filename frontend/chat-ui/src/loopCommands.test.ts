/**
 * Unit tests for the /loop slash-command parser (L2b).
 *
 * Covers create / status / cancel / cancel-with-id / help, the bare `/loop`
 * default, the live-loop helper, and — critically — that ordinary chat text
 * (including text that merely contains "loop") is passed through untouched.
 */
import { describe, expect, it } from 'vitest';
import type { LoopResponse } from '@xiaoguai/shared';
import {
  isLoopLive,
  parseLoopCommand,
  shortLoopId,
  TERMINAL_LOOP_STATUSES,
} from './loopCommands';

describe('parseLoopCommand', () => {
  it('parses `/loop <prompt>` as a create command preserving the prompt', () => {
    expect(parseLoopCommand('/loop check the deploy every morning')).toEqual({
      kind: 'create',
      prompt: 'check the deploy every morning',
    });
  });

  it('trims surrounding whitespace before parsing', () => {
    expect(parseLoopCommand('   /loop  poll the queue  ')).toEqual({
      kind: 'create',
      prompt: 'poll the queue',
    });
  });

  it('parses `/loop status`', () => {
    expect(parseLoopCommand('/loop status')).toEqual({ kind: 'status' });
  });

  it('parses `/loop cancel` with no id', () => {
    expect(parseLoopCommand('/loop cancel')).toEqual({ kind: 'cancel' });
  });

  it('parses `/loop cancel <id>` with an id', () => {
    expect(parseLoopCommand('/loop cancel abc-123')).toEqual({
      kind: 'cancel',
      id: 'abc-123',
    });
  });

  it('parses `/loop help` and bare `/loop` as help', () => {
    expect(parseLoopCommand('/loop help')).toEqual({ kind: 'help' });
    expect(parseLoopCommand('/loop')).toEqual({ kind: 'help' });
    expect(parseLoopCommand('  /loop  ')).toEqual({ kind: 'help' });
  });

  it('treats reserved subcommands case-insensitively', () => {
    expect(parseLoopCommand('/loop STATUS')).toEqual({ kind: 'status' });
    expect(parseLoopCommand('/loop Cancel')).toEqual({ kind: 'cancel' });
  });

  it('passes through ordinary text as { kind: "none" }', () => {
    expect(parseLoopCommand('hello there')).toEqual({ kind: 'none' });
    expect(parseLoopCommand('write a loop in python')).toEqual({ kind: 'none' });
    // `/loop` must be a whole word — `/looplike` is not a command.
    expect(parseLoopCommand('/looplike thing')).toEqual({ kind: 'none' });
    expect(parseLoopCommand('')).toEqual({ kind: 'none' });
  });
});

describe('loop helpers', () => {
  const base: LoopResponse = {
    id: 'abcdef12-3456-7890-0000-000000000000',
    session_id: 's',
    prompt: 'p',
    pacing_kind: 'fixed',
    interval_secs: 300,
    min_interval_secs: 30,
    max_interval_secs: 3600,
    max_ticks: 50,
    ttl_secs: 86400,
    max_total_tokens: 500000,
    status: 'active',
    created_by: 'owner',
    created_at: '',
    expires_at: '',
    next_tick_at: '',
    ticks_run: 0,
    consecutive_failures: 0,
  };

  it('isLoopLive is true for active / paused, false for terminal statuses', () => {
    expect(isLoopLive({ ...base, status: 'active' })).toBe(true);
    expect(isLoopLive({ ...base, status: 'paused' })).toBe(true);
    for (const status of TERMINAL_LOOP_STATUSES) {
      expect(isLoopLive({ ...base, status })).toBe(false);
    }
  });

  it('shortLoopId returns the first 8 chars', () => {
    expect(shortLoopId(base.id)).toBe('abcdef12');
  });
});
