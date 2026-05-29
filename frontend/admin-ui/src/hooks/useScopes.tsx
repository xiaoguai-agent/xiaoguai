/**
 * v1.8.0 (sprint-10b S10b-6) — `<ScopeProvider>` + `useScopes()` hook.
 *
 * Loads the bearer subject's effective scope list from
 * `GET /v1/admin/me/scopes` once on mount and caches it in a React
 * context for the lifetime of the app. The `<RequireScope>` component
 * (and any other gate-aware code) consumes this hook to decide what
 * to render.
 *
 * Fail-open contract (DEC-LLD-ADMIN-UI-002 + LLD-ADMIN-UI-001 §4.8):
 *   - When the endpoint returns 404 (older backend without
 *     `/admin/me/scopes`), the hook flips into `failOpen=true` mode.
 *     Callers of `hasScope()` then get `true` for every scope so the UI
 *     does not silently hide actions on operators who upgraded the
 *     frontend before the backend. A one-time `console.warn` makes the
 *     degradation visible to SRE.
 *   - When the endpoint returns 503 (backend wired but authz disabled),
 *     it actually responds 200 + the full vocabulary; the dev-mode
 *     fail-open lives in the Rust handler, not here.
 *   - Network errors are treated identically to 404 — better to show a
 *     button the user can't click (and get a 403 from the API) than to
 *     mysteriously hide it.
 */

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import { ApiError, type XiaoguaiClient } from '@xiaoguai/shared';
import { client as defaultClient } from '../client';

interface ScopeContextValue {
  /** True once the initial fetch has completed (success or fallback). */
  ready: boolean;
  /** True when we fell back to fail-open (no /me/scopes endpoint). */
  failOpen: boolean;
  /** Concrete scope list (empty when failOpen=true; gate against hasScope). */
  scopes: ReadonlySet<string>;
  /** Returns true when the scope is granted OR when failOpen is set. */
  hasScope(name: string): boolean;
}

const ScopeContext = createContext<ScopeContextValue | null>(null);

let failOpenWarned = false;

/** Module-internal — exposed for tests so they can flip the warning state. */
export function __resetFailOpenWarned(): void {
  failOpenWarned = false;
}

export interface ScopeProviderProps {
  children: ReactNode;
  /** Override for tests. Defaults to the shared XiaoguaiClient singleton. */
  client?: Pick<XiaoguaiClient, 'listMyScopes'>;
}

export function ScopeProvider({ children, client }: ScopeProviderProps): JSX.Element {
  const c = client ?? defaultClient;
  const [scopes, setScopes] = useState<ReadonlySet<string>>(new Set());
  const [ready, setReady] = useState(false);
  const [failOpen, setFailOpen] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const resp = await c.listMyScopes();
        if (cancelled) return;
        setScopes(new Set(resp.scopes));
        setFailOpen(false);
      } catch (err) {
        if (cancelled) return;
        // 404 (endpoint absent) or network failure → fail open. We do
        // NOT fail open on 401/403 — those are legitimate auth failures
        // and the rest of the app will already redirect / show a banner.
        const isMissing = err instanceof ApiError && err.status === 404;
        const isNetwork = !(err instanceof ApiError);
        if (isMissing || isNetwork) {
          setFailOpen(true);
          setScopes(new Set());
          if (!failOpenWarned) {
            failOpenWarned = true;
            // eslint-disable-next-line no-console
            console.warn(
              '[xiaoguai] /v1/admin/me/scopes unavailable — RequireScope falling open (DEC-LLD-ADMIN-UI-002).',
            );
          }
        } else {
          // 401/403/5xx — surface as "no scopes granted"; the rest of
          // the page will already have rendered an auth banner.
          setScopes(new Set());
          setFailOpen(false);
        }
      } finally {
        if (!cancelled) setReady(true);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [c]);

  const hasScope = useCallback(
    (name: string): boolean => {
      if (failOpen) return true;
      return scopes.has(name);
    },
    [scopes, failOpen],
  );

  const value = useMemo<ScopeContextValue>(
    () => ({ ready, failOpen, scopes, hasScope }),
    [ready, failOpen, scopes, hasScope],
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
