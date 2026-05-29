/**
 * v1.8.0 (sprint-10b S10b-6) — `<RequireScope>` gate component.
 *
 * Wraps any node that should only render when the bearer subject holds
 * a named scope. Resolves scopes via `<ScopeProvider>` (see
 * hooks/useScopes.tsx).
 *
 * Usage:
 *   <RequireScope name="personas.write">
 *     <button onClick={onSave}>Save</button>
 *   </RequireScope>
 *
 * Fail-open contract (DEC-LLD-ADMIN-UI-002 + LLD-ADMIN-UI-001 §4.8):
 *   - When the backend lacks `/v1/admin/me/scopes` (older deploy),
 *     `useScopes()` reports `failOpen=true` and `hasScope()` returns
 *     true for everything → we render `children` so operators upgrading
 *     the frontend ahead of the backend don't lose buttons.
 *   - When the endpoint is reachable but the scope is missing, we
 *     render `fallback` (default: nothing). The corresponding API call
 *     will still 403 server-side if the user fires the button via the
 *     URL bar — the gate is UX, not security.
 *
 * Pre-ready behaviour: while the initial fetch is in flight, we render
 * the fallback. This means buttons appear once scopes have loaded
 * rather than flashing in and being hidden — fewer visual surprises.
 */

import type { ReactNode } from 'react';
import { useScopes } from '../hooks/useScopes';

export interface RequireScopeProps {
  /** Scope name from the backend's `ADMIN_SCOPE_MAP`. */
  name: string;
  children: ReactNode;
  /** Rendered when the scope is not held. Defaults to `null` (hide). */
  fallback?: ReactNode;
}

export function RequireScope({
  name,
  children,
  fallback = null,
}: RequireScopeProps): JSX.Element {
  const { ready, hasScope } = useScopes();
  if (!ready) {
    return <>{fallback}</>;
  }
  return <>{hasScope(name) ? children : fallback}</>;
}
