/**
 * Unit tests for the Memory pane helpers (real /v1/memories contract).
 *
 * The helpers are exported from Memory.tsx and tested directly — no DOM
 * render needed. The type-checker enforces the component contract against
 * the shared wire types.
 */

import { describe, expect, it } from 'vitest';
import type { MemoryKind, MemoryRecord, RecalledMemory } from '@xiaoguai/shared';
import { MEMORY_KINDS } from '@xiaoguai/shared';
import {
  fmtDate,
  isoToLocalInput,
  kindBadgeClass,
  kindLabelKey,
  localInputToIso,
  preview,
  tagsFromRaw,
} from './Memory';

// ---------------------------------------------------------------------------
// preview
// ---------------------------------------------------------------------------

describe('preview', () => {
  it('returns the content as-is when short', () => {
    expect(preview('hello world')).toBe('hello world');
  });

  it('joins first two lines with a space', () => {
    expect(preview('line one\nline two\nline three')).toBe('line one line two');
  });

  it('truncates at 120 chars and adds ellipsis', () => {
    const long = 'x'.repeat(150);
    const p = preview(long);
    expect(p).toHaveLength(121); // 120 + ellipsis
    expect(p.endsWith('…')).toBe(true);
  });

  it('does not truncate when content is exactly 120 chars', () => {
    const exact = 'y'.repeat(120);
    expect(preview(exact)).toBe(exact);
  });
});

// ---------------------------------------------------------------------------
// fmtDate
// ---------------------------------------------------------------------------

describe('fmtDate', () => {
  it('returns em-dash for null', () => {
    expect(fmtDate(null)).toBe('—');
  });

  it('returns a non-empty string for a valid ISO date', () => {
    const out = fmtDate('2026-06-10T12:00:00Z');
    expect(out.length).toBeGreaterThan(0);
    expect(out).not.toBe('—');
  });
});

// ---------------------------------------------------------------------------
// kindBadgeClass / kindLabelKey
// ---------------------------------------------------------------------------

describe('kindBadgeClass', () => {
  it('returns chat class for facts', () => {
    expect(kindBadgeClass('facts')).toBe('kind-tag kind-tag-chat');
  });

  it('returns scheduled class for episodes', () => {
    expect(kindBadgeClass('episodes')).toBe('kind-tag kind-tag-scheduled');
  });

  it('returns im class for preferences', () => {
    expect(kindBadgeClass('preferences')).toBe('kind-tag kind-tag-im');
  });
});

describe('kindLabelKey', () => {
  it('maps every wire kind to an existing i18n key', () => {
    const expected: Record<MemoryKind, string> = {
      facts: 'pane.memory.type_fact',
      episodes: 'pane.memory.type_episode',
      preferences: 'pane.memory.type_preference',
    };
    for (const kind of MEMORY_KINDS) {
      expect(kindLabelKey(kind)).toBe(expected[kind]);
    }
  });
});

// ---------------------------------------------------------------------------
// tagsFromRaw
// ---------------------------------------------------------------------------

describe('tagsFromRaw', () => {
  it('splits on commas and trims whitespace', () => {
    expect(tagsFromRaw(' infra , compliance ,sla')).toEqual(['infra', 'compliance', 'sla']);
  });

  it('drops empty segments', () => {
    expect(tagsFromRaw('a,,b, ,')).toEqual(['a', 'b']);
  });

  it('returns empty array for blank input', () => {
    expect(tagsFromRaw('')).toEqual([]);
    expect(tagsFromRaw('   ')).toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// TTL conversion (datetime-local ⇄ RFC 3339)
// ---------------------------------------------------------------------------

describe('localInputToIso', () => {
  it('returns null for blank input', () => {
    expect(localInputToIso('')).toBeNull();
    expect(localInputToIso('   ')).toBeNull();
  });

  it('returns null for garbage input', () => {
    expect(localInputToIso('not-a-date')).toBeNull();
  });

  it('converts a datetime-local value to an ISO string', () => {
    const iso = localInputToIso('2026-12-31T08:30');
    expect(iso).not.toBeNull();
    expect(new Date(iso!).getTime()).toBe(new Date('2026-12-31T08:30').getTime());
    expect(iso!.endsWith('Z')).toBe(true);
  });
});

describe('isoToLocalInput', () => {
  it("returns '' for null", () => {
    expect(isoToLocalInput(null)).toBe('');
  });

  it("returns '' for garbage", () => {
    expect(isoToLocalInput('not-a-date')).toBe('');
  });

  it('round-trips with localInputToIso', () => {
    const local = '2026-12-31T08:30';
    const iso = localInputToIso(local);
    expect(isoToLocalInput(iso)).toBe(local);
  });
});

// ---------------------------------------------------------------------------
// Wire-shape sanity — the type-checker is the real assertion here
// ---------------------------------------------------------------------------

describe('wire shapes', () => {
  it('MemoryRecord uses kind + ttl_at (no tenant/agent fields)', () => {
    const rec: MemoryRecord = {
      id: '11111111-1111-1111-1111-111111111111',
      kind: 'facts',
      content: 'The primary data center is Frankfurt.',
      tags: ['infra', 'source:imported'],
      ttl_at: null,
      created_at: '2026-06-01T09:00:00Z',
      last_recalled_at: null,
      recall_count: 0,
    };
    expect(rec.kind).toBe('facts');
    expect('tenant_id' in rec).toBe(false);
  });

  it('RecalledMemory wraps a full memory with a score', () => {
    const hit: RecalledMemory = {
      memory: {
        id: '22222222-2222-2222-2222-222222222222',
        kind: 'preferences',
        content: 'Prefers concise bullet-point summaries.',
        tags: [],
        ttl_at: null,
        created_at: '2026-06-01T09:00:00Z',
        last_recalled_at: '2026-06-10T09:00:00Z',
        recall_count: 3,
      },
      score: 0.92,
    };
    expect(hit.score).toBeGreaterThan(0);
    expect(hit.memory.kind).toBe('preferences');
  });
});
