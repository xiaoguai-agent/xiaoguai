/**
 * Unit tests for cost formatting utilities — v1.1.1.1.
 */

import { describe, expect, it } from 'vitest';
import { formatCents, sumCosts } from './cost';

describe('formatCents', () => {
  it('formats zero cents as $0.0000', () => {
    expect(formatCents(0)).toBe('$0.0000');
  });

  it('formats sub-dollar amounts with 4 decimal places', () => {
    // 1 cent = $0.01
    expect(formatCents(1)).toBe('$0.0100');
    // 27 cents = $0.27
    expect(formatCents(27)).toBe('$0.2700');
    // 99 cents = $0.99
    expect(formatCents(99)).toBe('$0.9900');
  });

  it('formats $1.00 and above with 2 decimal places', () => {
    // 100 cents = $1.00
    expect(formatCents(100)).toBe('$1.00');
    // 150 cents = $1.50  (haiku 500 in / 200 out)
    expect(formatCents(150)).toBe('$1.50');
    // 750 cents = $7.50  (gpt-4o 1000 in / 500 out)
    expect(formatCents(750)).toBe('$7.50');
    // 1500 cents = $15.00
    expect(formatCents(1500)).toBe('$15.00');
  });

  it('formats large values correctly', () => {
    // 75000 cents = $750.00 (e.g. large Opus run)
    expect(formatCents(75000)).toBe('$750.00');
  });
});

describe('sumCosts', () => {
  it('returns 0 for an empty array', () => {
    expect(sumCosts([])).toBe(0);
  });

  it('sums all costs when all are non-null', () => {
    expect(sumCosts([250, 500])).toBe(750);
    expect(sumCosts([0, 0, 0])).toBe(0);
    expect(sumCosts([150])).toBe(150);
  });

  it('returns null when any cost is null (partial rates)', () => {
    expect(sumCosts([250, null])).toBeNull();
    expect(sumCosts([null, 500])).toBeNull();
    expect(sumCosts([null])).toBeNull();
    expect(sumCosts([250, null, 100])).toBeNull();
  });
});
