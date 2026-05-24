/**
 * Cost formatting utilities — v1.1.1.1.
 *
 * Kept separate from the Usage pane component so they can be unit-tested
 * without loading React.
 */

/**
 * Format a cost in cents as a USD dollar string.
 *
 * Uses 4 decimal places for amounts below $1.00 so that sub-cent amounts
 * (e.g. $0.0027 for a small Haiku call) are readable. Large amounts
 * (≥ $1.00) use 2 decimal places for compactness.
 *
 * @example formatCents(0)   → "$0.0000"
 * @example formatCents(1)   → "$0.0100"
 * @example formatCents(150) → "$1.50"
 * @example formatCents(750) → "$7.50"
 */
export function formatCents(cents: number): string {
  const dollars = cents / 100;
  const decimals = dollars < 1 ? 4 : 2;
  return `$${dollars.toFixed(decimals)}`;
}

/**
 * Compute the total cost in cents from a list of nullable per-row costs.
 *
 * Returns `null` when ANY row has a `null` cost (partial / unconfigured
 * rates), matching the API's partial-cost sentinel semantics.
 */
export function sumCosts(costs: ReadonlyArray<number | null>): number | null {
  let total = 0;
  for (const c of costs) {
    if (c === null) return null;
    total += c;
  }
  return total;
}
