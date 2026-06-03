/**
 * v1.8.0 (sprint-10b S10b-6) — `<ScopeProvider>` + `useScopes()` hook.
 *
 * Originally this loaded the bearer subject's effective scope list from
 * `GET /v1/admin/me/scopes`. Under the single-user pivot (DEC-033) that
 * endpoint is gone — there is one owner who has every scope. The provider
 * now resolves immediately into a fail-open state: `ready=true`,
 * `failOpen=true`, and `hasScope()` always returns true.
 *
 * The `useScopes()` / `<ScopeProvider>` / `<RequireScope>` API surface is
 * kept intact so panes and gates keep compiling and every gate renders.
 */

import {
  createContext,
  useContext,
  useMemo,
  type ReactNode,
} from 'react';

interface ScopeContextValue {
  /** True once the initial fetch has completed (success or fallback). */
  ready: boolean;
  /** True when we fell back to fail-open (single-owner: always true). */
  failOpen: boolean;
  /** Concrete scope list (empty under fail-open; gate against hasScope). */
  scopes: ReadonlySet<string>;
  /** Returns true when the scope is granted OR when failOpen is set. */
  hasScope(name: string): boolean;
}

const ScopeContext = createContext<ScopeContextValue | null>(null);

/** Retained for source compatibility with callers/tests that imported it. */
export function __resetFailOpenWarned(): void {
  // no-op: there is no fetch and no one-time warning anymore.
}

export interface ScopeProviderProps {
  children: ReactNode;
  /**
   * Retained for source compatibility with existing callers/tests. Under
   * the single-user pivot scopes are never fetched, so this is unused.
   */
  client?: unknown;
}

const EMPTY_SCOPES: ReadonlySet<string> = new Set();

export function ScopeProvider({ children }: ScopeProviderProps): JSX.Element {
  // Single owner has every scope: resolve immediately, fail open.
  const value = useMemo<ScopeContextValue>(
    () => ({
      ready: true,
      failOpen: true,
      scopes: EMPTY_SCOPES,
      hasScope: () => true,
    }),
    [],
  );

  return <ScopeContext.Provider value={value}>{children}</ScopeContext.Provider>;
}

/**
 * Read the current scope context. Throws when called outside a
 * `<ScopeProvider>` — that's a programming error, not a runtime
 * condition.
 */
export function useScopes(): ScopeContextValue {
  const ctx = useContext(ScopeContext);
  if (!ctx) {
    throw new Error('useScopes() must be called inside a <ScopeProvider>');
  }
  return ctx;
}
