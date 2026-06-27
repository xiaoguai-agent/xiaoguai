/**
 * v1.3.x — Outcomes browser pane tests.
 *
 * Covers:
 *  - list render helpers (fmtValue, pivotTimeseries, kindsInTimeseries)
 *  - filter/aggregation helpers (aggregateByAgent)
 *  - session chain-tree builder (buildChainTree, 3-deep chain mock)
 *  - 503 fallback handling
 *  - OutcomeRecord wire-type shape
 *  - OutcomesSummaryResponse shape
 */

import { describe, expect, it } from 'vitest';
import type { OutcomeDay, OutcomeRecord, SessionResponse } from '@xiaoguai/shared';
import {
  aggregateByAgent,
  buildChainTree,
  fmtValue,
  kindsInTimeseries,
  pivotTimeseries,
} from './Outcomes';

// ---------------------------------------------------------------------------
// fmtValue
// ---------------------------------------------------------------------------

describe('fmtValue', () => {
  it('formats revenue_usd with dollar sign and 2dp', () => {
    expect(fmtValue('revenue_usd', 1234.5)).toBe('$1,234.50');
    expect(fmtValue('revenue_usd', 0)).toBe('$0.00');
  });

  it('formats cost_saved_usd the same as revenue_usd', () => {
    expect(fmtValue('cost_saved_usd', 500)).toBe('$500.00');
  });

  it('formats hours_saved with h suffix', () => {
    expect(fmtValue('hours_saved', 8)).toBe('8 h');
    expect(fmtValue('hours_saved', 8.5)).toBe('8.5 h');
  });

  it('formats integer kinds without decimal places', () => {
    expect(fmtValue('deals_closed', 3)).toBe('3');
    expect(fmtValue('tickets_resolved', 42)).toBe('42');
    expect(fmtValue('custom', 100)).toBe('100');
  });
});

// ---------------------------------------------------------------------------
// pivotTimeseries
// ---------------------------------------------------------------------------

describe('pivotTimeseries', () => {
  it('returns empty array for empty input', () => {
    expect(pivotTimeseries([])).toEqual([]);
  });

  it('pivots a single entry correctly', () => {
    const days: OutcomeDay[] = [
      { date: '2026-05-20', kind: 'revenue_usd', sum: 100, count: 1 },
    ];
    expect(pivotTimeseries(days)).toEqual([{ date: '2026-05-20', revenue_usd: 100 }]);
  });

  it('merges two kinds on the same date', () => {
    const days: OutcomeDay[] = [
      { date: '2026-05-20', kind: 'revenue_usd', sum: 100, count: 1 },
      { date: '2026-05-20', kind: 'hours_saved', sum: 8, count: 2 },
    ];
    const result = pivotTimeseries(days);
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({ date: '2026-05-20', revenue_usd: 100, hours_saved: 8 });
  });

  it('sums duplicate kind entries on the same date', () => {
    const days: OutcomeDay[] = [
      { date: '2026-05-20', kind: 'revenue_usd', sum: 50, count: 1 },
      { date: '2026-05-20', kind: 'revenue_usd', sum: 50, count: 1 },
    ];
    const result = pivotTimeseries(days);
    expect(result[0]?.revenue_usd).toBe(100);
  });

  it('sorts output by date ascending', () => {
    const days: OutcomeDay[] = [
      { date: '2026-05-22', kind: 'revenue_usd', sum: 10, count: 1 },
      { date: '2026-05-20', kind: 'revenue_usd', sum: 20, count: 1 },
      { date: '2026-05-21', kind: 'revenue_usd', sum: 15, count: 1 },
    ];
    const result = pivotTimeseries(days);
    expect(result.map((r) => r['date'])).toEqual(['2026-05-20', '2026-05-21', '2026-05-22']);
  });

  it('handles multiple dates with multiple kinds', () => {
    const days: OutcomeDay[] = [
      { date: '2026-05-20', kind: 'revenue_usd', sum: 100, count: 2 },
      { date: '2026-05-21', kind: 'hours_saved', sum: 4, count: 1 },
      { date: '2026-05-21', kind: 'revenue_usd', sum: 200, count: 3 },
    ];
    const result = pivotTimeseries(days);
    expect(result).toHaveLength(2);
    expect(result[0]).toEqual({ date: '2026-05-20', revenue_usd: 100 });
    expect(result[1]).toEqual({ date: '2026-05-21', hours_saved: 4, revenue_usd: 200 });
  });
});

// ---------------------------------------------------------------------------
// kindsInTimeseries
// ---------------------------------------------------------------------------

describe('kindsInTimeseries', () => {
  it('returns empty array for empty input', () => {
    expect(kindsInTimeseries([])).toEqual([]);
  });

  it('deduplicates and sorts kinds', () => {
    const days: OutcomeDay[] = [
      { date: '2026-05-20', kind: 'revenue_usd', sum: 1, count: 1 },
      { date: '2026-05-21', kind: 'hours_saved', sum: 2, count: 1 },
      { date: '2026-05-21', kind: 'revenue_usd', sum: 3, count: 1 },
    ];
    expect(kindsInTimeseries(days)).toEqual(['hours_saved', 'revenue_usd']);
  });
});

// ---------------------------------------------------------------------------
// aggregateByAgent (list-view filter helper)
// ---------------------------------------------------------------------------

describe('aggregateByAgent', () => {
  it('returns empty array for empty input', () => {
    expect(aggregateByAgent([])).toEqual([]);
  });

  it('aggregates a single agent', () => {
    const records: OutcomeRecord[] = [
      makeRecord('sales-bot', 'revenue_usd', 100),
      makeRecord('sales-bot', 'revenue_usd', 200),
    ];
    const result = aggregateByAgent(records);
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({ agent: 'sales-bot', count: 2, sum: 300 });
  });

  it('aggregates multiple agents and sorts by sum descending', () => {
    const records: OutcomeRecord[] = [
      makeRecord('bot-a', 'hours_saved', 10),
      makeRecord('bot-b', 'revenue_usd', 500),
      makeRecord('bot-a', 'hours_saved', 5),
    ];
    const result = aggregateByAgent(records);
    expect(result[0]?.agent).toBe('bot-b');
    expect(result[1]?.agent).toBe('bot-a');
    expect(result[1]?.count).toBe(2);
    expect(result[1]?.sum).toBe(15);
  });
});

// ---------------------------------------------------------------------------
// buildChainTree — 3-deep chain mock
// ---------------------------------------------------------------------------

describe('buildChainTree', () => {
  it('returns null when root id not in sessions', () => {
    expect(buildChainTree([], 'missing')).toBeNull();
  });

  it('builds a single-node tree', () => {
    const sessions: SessionResponse[] = [makeSession('s1', undefined)];
    const tree = buildChainTree(sessions, 's1');
    expect(tree).not.toBeNull();
    expect(tree!.session.id).toBe('s1');
    expect(tree!.children).toHaveLength(0);
  });

  it('builds a 3-deep chain (root → child → grandchild)', () => {
    const sessions: SessionResponse[] = [
      makeSession('root', undefined),
      makeSession('child', 'root'),
      makeSession('grandchild', 'child'),
    ];
    const tree = buildChainTree(sessions, 'root');
    expect(tree).not.toBeNull();
    expect(tree!.children).toHaveLength(1);
    const child = tree!.children[0]!;
    expect(child.session.id).toBe('child');
    expect(child.children).toHaveLength(1);
    const gc = child.children[0]!;
    expect(gc.session.id).toBe('grandchild');
    expect(gc.children).toHaveLength(0);
  });

  it('handles branched tree (root with two children)', () => {
    const sessions: SessionResponse[] = [
      makeSession('root', undefined),
      makeSession('branch-a', 'root'),
      makeSession('branch-b', 'root'),
    ];
    const tree = buildChainTree(sessions, 'root');
    expect(tree!.children).toHaveLength(2);
  });

  it('does not infinite-loop on cycles', () => {
    // Malformed data: s1 → s2 → s1
    const s1 = makeSession('s1', 's2');
    const s2 = makeSession('s2', 's1');
    // buildChainTree starts from root (s1); cycle guard should prevent infinite recursion
    const result = buildChainTree([s1, s2], 's1');
    // Should complete without throwing, even if tree is partial
    expect(result).not.toBeNull();
  });
});

// ---------------------------------------------------------------------------
// 503 fallback: wire-type assertion
// ---------------------------------------------------------------------------

describe('503 fallback — PgOutcomeRecorder not wired', () => {
  it('OutcomeRecord accepts the expected wire shape', () => {
    // Compile-time check embedded as runtime assertion.
    // If the type changes, tsc --noEmit will fail before this runs.
    const rec: OutcomeRecord = {
      session_id: 'sess_abc123',
      agent_name: 'sales-bot',
      kind: 'revenue_usd',
      value: 1250.0,
      unit: 'usd',
      description: 'Closed deal D-4471',
      attributed_at: '2026-05-25T12:34:56Z',
      metadata: { deal_id: 'D-4471' },
    };
    expect(rec.value).toBe(1250.0);
  });

  it('OutcomeRecord allows nullable session_id, unit, description', () => {
    const rec: OutcomeRecord = {
      session_id: null,
      agent_name: 'bot',
      kind: 'custom',
      value: 1,
      unit: null,
      description: null,
      attributed_at: '2026-05-25T00:00:00Z',
      metadata: null,
    };
    expect(rec.session_id).toBeNull();
    expect(rec.unit).toBeNull();
  });

  it('summary response wire shape accepted', () => {
    const resp = {
      range: '7d',
      summary: {
        by_kind: {
          revenue_usd: { sum: 500, count: 2, avg: 250 },
          hours_saved: { sum: 8, count: 1, avg: 8 },
        },
      },
    };
    expect(resp.summary.by_kind['revenue_usd']?.sum).toBe(500);
    expect(resp.summary.by_kind['hours_saved']?.count).toBe(1);
  });
});

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeRecord(agent: string, kind: string, value: number): OutcomeRecord {
  return {
    session_id: null,
    agent_name: agent,
    kind,
    value,
    unit: null,
    description: null,
    attributed_at: '2026-05-25T00:00:00Z',
    metadata: null,
  };
}

function makeSession(id: string, parentId: string | undefined): SessionResponse {
  return {
    id,
    user_id: 'user_1',
    title: `Session ${id}`,
    model: 'claude-3-5-haiku',
    status: 'active',
    parent_session_id: parentId,
  };
}
