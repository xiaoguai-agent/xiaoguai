import { useCallback, useEffect, useState } from 'react';

/**
 * Shared async-loading state machine for admin panes (DEC-041, frontend half).
 *
 * Replaces the per-pane `useState<T | null>(null)` + `useState<string | null>`
 * + `useEffect`-with-cancellation boilerplate that panes hand-rolled three
 * different ways (null-check / boolean-triple / ad-hoc state machine). Loads
 * once on mount — and again whenever `deps` change or `reload()` is called —
 * tracking a single loading/data/error machine.
 *
 * The `loader` is intentionally NOT a dependency of the effect: callers pass
 * an inline closure and control re-runs through `deps`, mirroring the
 * established "load on mount" panes. Stale loads are dropped via a cancel flag.
 */
export interface UseAsyncResult<T> {
  /** Loaded value, or `null` before the first successful load. */
  data: T | null;
  /** Error message from the most recent failed load, else `null`. */
  error: string | null;
  /** True while a load (initial or reload) is in flight. */
  loading: boolean;
  /** Re-run the loader — e.g. an `<ErrorBanner>` retry button. */
  reload: () => void;
}

export function useAsyncState<T>(
  loader: () => Promise<T>,
  deps: readonly unknown[] = [],
): UseAsyncResult<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [nonce, setNonce] = useState(0);

  const reload = useCallback(() => setNonce((n) => n + 1), []);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    void (async () => {
      try {
        const result = await loader();
        if (!cancelled) {
          setData(result);
          setLoading(false);
        }
      } catch (err) {
        if (!cancelled) {
          setError((err as Error).message);
          setLoading(false);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
    // `loader` is deliberately excluded — callers control re-runs via `deps`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, nonce]);

  return { data, error, loading, reload };
}
