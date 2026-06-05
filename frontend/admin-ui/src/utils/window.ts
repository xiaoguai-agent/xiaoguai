/**
 * Format a HotL policy window (in seconds) as a compact human string.
 *
 * Shared by the HotL policy table and the trust-tier panel so the same window
 * always renders identically (previously the two had diverging copies — one
 * showed `1d`, the other `24h` for the same value).
 */
export function fmtWindow(seconds: number): string {
  if (seconds % 86400 === 0) return `${seconds / 86400}d`;
  if (seconds % 3600 === 0) return `${seconds / 3600}h`;
  if (seconds % 60 === 0) return `${seconds / 60}m`;
  return `${seconds}s`;
}
