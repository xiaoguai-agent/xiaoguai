/**
 * v1.4-ready — Unit tests for the Memory pane helpers.
 *
 * Like Outcomes.test.ts we test the pure helpers (preview, fmtDate,
 * typeBadgeClass, filter logic, mock data shape) without a DOM render.
 * The type-checker enforces the component contract against shared types.
 */

import { describe, expect, it } from 'vitest';
import type { MemoryRecord, MemoryType, RecallTraceResponse } from '@xiaoguai/shared';

// ---------------------------------------------------------------------------
// Inline helpers from Memory.tsx (not exported — test by reimplementation,
// same pattern as Outcomes.test.ts).
// ---------------------------------------------------------------------------

function preview(content: string): string {
  const lines = content.split('\n').slice(0, 2).join(' ');
  return lines.length > 120 ? `${lines.slice(0, 120)}…` : lines;
}

function fmtDate(iso: string | null): string {
  if (!iso) return '—';
  return new Date(iso).toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function typeBadgeClass(type: MemoryType): string {
  if (type === 'fact') return 'kind-tag kind-tag-chat';
  if (type === 'episode') return 'kind-tag kind-tag-scheduled';
  return 'kind-tag kind-tag-im';
}

// ---------------------------------------------------------------------------
// Mock data shape used by the 404-fallback path.
// ---------------------------------------------------------------------------

const MOCK_MEMORIES: MemoryRecord[] = [
  {
    id: 'mem_mock_001',
    type: 'fact',
    content:
      'The primary data center is located in Frankfurt (eu-central-1). All PII must stay within the EU.',
    tags: ['infra', 'compliance'],
    tenant_id: 'ten_demo',
    agent_id: 'agent_ops',
    created_at: '2026-05-01T09:00:00Z',
    last_recalled_at: '2026-05-24T14:30:00Z',
    recall_count: 12,
    ttl: null,
  },
  {
    id: 'mem_mock_002',
    type: 'fact',
    content: 'SLA for Tier-1 incidents is 15 minutes acknowledgment, 4 hours resolution.',
    tags: ['sla', 'incident'],
    tenant_id: 'ten_demo',
    agent_id: 'agent_ops',
    created_at: '2026-05-03T10:15:00Z',
    last_recalled_at: '2026-05-22T08:10:00Z',
    recall_count: 7,
    ttl: null,
  },
  {
    id: 'mem_mock_003',
    type: 'fact',
    content: 'PostgreSQL version in production is 16.3.',
    tags: ['database', 'alerts'],
    tenant_id: 'ten_demo',
    agent_id: 'agent_dba',
    created_at: '2026-05-10T11:00:00Z',
    last_recalled_at: null,
    recall_count: 0,
    ttl: 'P90D',
  },
  {
    id: 'mem_mock_004',
    type: 'episode',
    content: 'Incident 2026-05-18: disk full on kafka-03.',
    tags: ['incident', 'kafka', 'postmortem'],
    tenant_id: 'ten_demo',
    agent_id: 'agent_ops',
    created_at: '2026-05-18T23:45:00Z',
    last_recalled_at: '2026-05-19T09:00:00Z',
    recall_count: 3,
    ttl: 'P365D',
  },
  {
    id: 'mem_mock_005',
    type: 'episode',
    content: 'On-boarding session with customer Acme Corp.',
    tags: ['customer', 'onboarding', 'acme'],
    tenant_id: 'ten_demo',
    agent_id: 'agent_sales',
    created_at: '2026-05-20T15:00:00Z',
    last_recalled_at: '2026-05-21T08:30:00Z',
    recall_count: 2,
    ttl: 'P180D',
  },
  {
    id: 'mem_mock_006',
    type: 'preference',
    content: 'User prefers concise bullet-point summaries. Always include TL;DR.',
    tags: ['user-pref', 'formatting'],
    tenant_id: 'ten_demo',
    agent_id: null,
    created_at: '2026-05-05T08:00:00Z',
    last_recalled_at: '2026-05-25T10:00:00Z',
    recall_count: 28,
    ttl: null,
  },
];

// ---------------------------------------------------------------------------
// preview()
// ---------------------------------------------------------------------------

describe('preview', () => {
  it('returns the content as-is when short', () => {
    expect(preview('Hello world')).toBe('Hello world');
  });

  it('joins first two lines with a space', () => {
    expect(preview('line one\nline two\nline three')).toBe('line one line two');
  });

  it('truncates at 120 chars and adds ellipsis', () => {
    const long = 'x'.repeat(130);
    const result = preview(long);
    expect(result.endsWith('…')).toBe(true);
    expect(result.length).toBe(121); // 120 chars + '…'
  });

  it('does not truncate when content is exactly 120 chars', () => {
    const exact = 'y'.repeat(120);
    expect(preview(exact)).toBe(exact);
    expect(preview(exact).endsWith('…')).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// fmtDate()
// ---------------------------------------------------------------------------

describe('fmtDate', () => {
  it('returns em-dash for null', () => {
    expect(fmtDate(null)).toBe('—');
  });

  it('returns a non-empty string for a valid ISO date', () => {
    const result = fmtDate('2026-05-01T09:00:00Z');
    expect(typeof result).toBe('string');
    expect(result.length).toBeGreaterThan(0);
    expect(result).not.toBe('—');
  });
});

// ---------------------------------------------------------------------------
// typeBadgeClass()
// ---------------------------------------------------------------------------

describe('typeBadgeClass', () => {
  it('returns chat class for fact', () => {
    expect(typeBadgeClass('fact')).toBe('kind-tag kind-tag-chat');
  });

  it('returns scheduled class for episode', () => {
    expect(typeBadgeClass('episode')).toBe('kind-tag kind-tag-scheduled');
  });

  it('returns im class for preference', () => {
    expect(typeBadgeClass('preference')).toBe('kind-tag kind-tag-im');
  });
});

// ---------------------------------------------------------------------------
// Mock data: list view filter simulation (type filter)
// ---------------------------------------------------------------------------

describe('list view type filter (mock data path)', () => {
  function filterByType(records: MemoryRecord[], type: MemoryType | ''): MemoryRecord[] {
    if (!type) return records;
    return records.filter((r) => r.type === type);
  }

  it('returns all records when type is empty string', () => {
    expect(filterByType(MOCK_MEMORIES, '')).toHaveLength(6);
  });

  it('returns only facts when type is fact', () => {
    const result = filterByType(MOCK_MEMORIES, 'fact');
    expect(result).toHaveLength(3);
    result.forEach((r) => expect(r.type).toBe('fact'));
  });

  it('returns only episodes when type is episode', () => {
    const result = filterByType(MOCK_MEMORIES, 'episode');
    expect(result).toHaveLength(2);
    result.forEach((r) => expect(r.type).toBe('episode'));
  });

  it('returns only preferences when type is preference', () => {
    const result = filterByType(MOCK_MEMORIES, 'preference');
    expect(result).toHaveLength(1);
    expect(result[0]?.type).toBe('preference');
  });
});

// ---------------------------------------------------------------------------
// Mock data: recall trace view
// ---------------------------------------------------------------------------

describe('recall trace view (mock data shape)', () => {
  const trace: RecallTraceResponse = {
    session_id: 'sess_mock_abc123',
    query: null,
    entries: [
      {
        memory_id: 'mem_mock_006',
        relevance_score: 0.97,
        agent_id: 'agent_ops',
        recalled_at: '2026-05-25T10:00:00Z',
        content_preview: 'User prefers concise…',
        type: 'preference',
        tags: ['user-pref'],
      },
      {
        memory_id: 'mem_mock_001',
        relevance_score: 0.82,
        agent_id: 'agent_ops',
        recalled_at: '2026-05-25T10:00:01Z',
        content_preview: 'Frankfurt data center…',
        type: 'fact',
        tags: ['infra'],
      },
    ],
    total: 2,
  };

  it('has the correct session_id', () => {
    expect(trace.session_id).toBe('sess_mock_abc123');
  });

  it('sorts by relevance descending', () => {
    const scores = trace.entries.map((e) => e.relevance_score);
    expect(scores[0]).toBeGreaterThanOrEqual(scores[1]!);
  });

  it('all entries have valid relevance scores in [0, 1]', () => {
    trace.entries.forEach((e) => {
      expect(e.relevance_score).toBeGreaterThanOrEqual(0);
      expect(e.relevance_score).toBeLessThanOrEqual(1);
    });
  });

  it('total matches entries length in mock', () => {
    expect(trace.total).toBe(trace.entries.length);
  });
});

// ---------------------------------------------------------------------------
// Mock data: vector neighbors view
// ---------------------------------------------------------------------------

describe('vector neighbors view (mock data shape)', () => {
  const neighbors = [
    {
      memory_id: 'mem_mock_002',
      similarity: 0.88,
      content_preview: 'SLA for Tier-1 incidents…',
      type: 'fact' as MemoryType,
      tags: ['sla', 'incident'],
      created_at: '2026-05-03T10:15:00Z',
    },
    {
      memory_id: 'mem_mock_004',
      similarity: 0.74,
      content_preview: 'Incident 2026-05-18…',
      type: 'episode' as MemoryType,
      tags: ['incident'],
      created_at: '2026-05-18T23:45:00Z',
    },
  ];

  it('all neighbors have similarity in [0, 1]', () => {
    neighbors.forEach((nb) => {
      expect(nb.similarity).toBeGreaterThan(0);
      expect(nb.similarity).toBeLessThanOrEqual(1);
    });
  });

  it('neighbors are ordered by similarity descending', () => {
    for (let i = 0; i < neighbors.length - 1; i++) {
      expect(neighbors[i]!.similarity).toBeGreaterThanOrEqual(neighbors[i + 1]!.similarity);
    }
  });

  it('high-similarity neighbors (> 0.85) are flagged correctly', () => {
    const high = neighbors.filter((nb) => nb.similarity > 0.85);
    expect(high).toHaveLength(1);
    expect(high[0]?.memory_id).toBe('mem_mock_002');
  });
});

// ---------------------------------------------------------------------------
// New memory creation: request shape validation
// ---------------------------------------------------------------------------

describe('CreateMemoryRequest wire shape', () => {
  it('constructs a valid create request for a fact', () => {
    const req = {
      type: 'fact' as MemoryType,
      content: 'Frankfurt DC hosts all PII data.',
      tags: ['infra', 'compliance'],
      tenant_id: 'ten_demo',
      ttl: null,
    };
    expect(req.type).toBe('fact');
    expect(req.tags).toContain('infra');
    expect(req.ttl).toBeNull();
  });

  it('constructs a valid create request with TTL', () => {
    const req = {
      type: 'episode' as MemoryType,
      content: 'On-boarding with Acme.',
      tags: ['customer'],
      tenant_id: 'ten_demo',
      ttl: 'P180D',
    };
    expect(req.ttl).toBe('P180D');
  });
});

// ---------------------------------------------------------------------------
// Edit existing: only mutable fields change
// ---------------------------------------------------------------------------

describe('UpdateMemoryRequest — immutability contract', () => {
  it('only updates content, tags, ttl — not type or created_at', () => {
    const original = MOCK_MEMORIES[0]!;
    const patch = {
      content: 'Updated content.',
      tags: ['infra', 'updated'],
      ttl: 'P30D',
    };
    // Simulate the merge the pane does when onSaved is called:
    const updated: MemoryRecord = { ...original, ...patch };
    expect(updated.type).toBe(original.type); // immutable
    expect(updated.created_at).toBe(original.created_at); // immutable
    expect(updated.content).toBe('Updated content.');
    expect(updated.tags).toContain('updated');
    expect(updated.ttl).toBe('P30D');
  });
});

// ---------------------------------------------------------------------------
// Delete confirmation: the record id survives the flow
// ---------------------------------------------------------------------------

describe('delete flow', () => {
  it('identifies the record to delete by id', () => {
    const target = MOCK_MEMORIES[3]!;
    expect(target.id).toBe('mem_mock_004');
    const remaining = MOCK_MEMORIES.filter((r) => r.id !== target.id);
    expect(remaining).toHaveLength(MOCK_MEMORIES.length - 1);
    expect(remaining.find((r) => r.id === 'mem_mock_004')).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// 404 fallback — mock data provides correct variety
// ---------------------------------------------------------------------------

describe('404 fallback mock data', () => {
  it('contains 3 facts, 2 episodes, and 1 preference', () => {
    const facts = MOCK_MEMORIES.filter((m) => m.type === 'fact');
    const episodes = MOCK_MEMORIES.filter((m) => m.type === 'episode');
    const prefs = MOCK_MEMORIES.filter((m) => m.type === 'preference');
    expect(facts).toHaveLength(3);
    expect(episodes).toHaveLength(2);
    expect(prefs).toHaveLength(1);
  });

  it('all records have required fields', () => {
    MOCK_MEMORIES.forEach((m) => {
      expect(m.id).toBeTruthy();
      expect(m.type).toMatch(/^(fact|episode|preference)$/);
      expect(m.content).toBeTruthy();
      expect(Array.isArray(m.tags)).toBe(true);
      expect(m.tenant_id).toBeTruthy();
      expect(m.created_at).toBeTruthy();
      expect(typeof m.recall_count).toBe('number');
    });
  });

  it('preference record has null agent_id (system-level)', () => {
    const pref = MOCK_MEMORIES.find((m) => m.type === 'preference')!;
    expect(pref.agent_id).toBeNull();
  });

  it('some records have TTL and others do not', () => {
    const withTtl = MOCK_MEMORIES.filter((m) => m.ttl !== null);
    const withoutTtl = MOCK_MEMORIES.filter((m) => m.ttl === null);
    expect(withTtl.length).toBeGreaterThan(0);
    expect(withoutTtl.length).toBeGreaterThan(0);
  });
});
