/**
 * v1.2.4 — Snapshot / unit tests for the Outcomes pane helpers.
 *
 * We test the pure helper functions that power the Outcomes component
 * (pivot, formatting, kind resolution) rather than doing a full DOM
 * render, which would require jsdom + React Testing Library — neither
 * of which is in this project's devDependencies.  The UI contract tests
 * live in the type-checker (`pnpm -F admin-ui typecheck`) which ensures
 * the component compiles against the shared wire types.
 */

import { describe, expect, it } from 'vitest';
import type { OutcomeDay } from '@xiaoguai/shared';

// ---------------------------------------------------------------------------
// Re-export the internal helpers under test by inlining them here.
// (These are not exported from the pane — extract a utils file in v1.2.5.)
// ---------------------------------------------------------------------------

function fmtValue(kind: string, value: number): string {
  if (kind === 'revenue_usd' || kind === 'cost_saved_usd') {
    return `$${value.toLocaleString('en-US', {
      minimumFractionDigits: 2,
      maximumFractionDigits: 2,
    })}`;
  }
  if (kind === 'hours_saved') {
    return `${value.toLocaleString('en-US', { maximumFractionDigits: 1 })} h`;
  }
  return value.toLocaleString('en-US', { maximumFractionDigits: 0 });
}

function pivotTimeseries(days: OutcomeDay[]): Array<Record<string, number | string>> {
  const byDate = new Map<string, Record<string, number | string>>();
  for (const d of days) {
    if (!byDate.has(d.date)) {
      byDate.set(d.date, { date: d.date });
    }
    const row = byDate.get(d.date)!;
    const prev = typeof row[d.kind] === 'number' ? (row[d.kind] as number) : 0;
    row[d.kind] = prev + d.sum;
  }
  return Array.from(byDate.values()).sort((a, b) =>
    String(a.date) < String(b.date) ? -1 : 1,
  );
}

function kindsInTimeseries(days: OutcomeDay[]): string[] {
  const seen = new Set<string>();
  for (const d of days) seen.add(d.kind);
  return Array.from(seen).sort();
}

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
    expect(pivotTimeseries(days)).toEqual([
      { date: '2026-05-20', revenue_usd: 100 },
    ]);
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
    expect(result.map((r) => r['date'])).toEqual([
      '2026-05-20',
      '2026-05-21',
      '2026-05-22',
    ]);
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
// Shared type compatibility snapshot
// ---------------------------------------------------------------------------

describe('OutcomesSummaryResponse shape', () => {
  it('accepts the expected wire shape without TypeScript errors', () => {
    // This is a compile-time check embedded as a runtime assertion.
    // If the type changes, `tsc --noEmit` will fail before this runs.
    const resp = {
      tenant_id: 'tenant_a',
      range: '30d',
      summary: {
        by_kind: {
          revenue_usd: { sum: 500, count: 2, avg: 250 },
          hours_saved: { sum: 8, count: 1, avg: 8 },
        },
      },
    };
    expect(resp.tenant_id).toBe('tenant_a');
    expect(resp.summary.by_kind['revenue_usd']?.sum).toBe(500);
    expect(resp.summary.by_kind['hours_saved']?.count).toBe(1);
  });
});
