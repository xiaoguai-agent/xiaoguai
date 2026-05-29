/**
 * Sprint-10b S10b-9 — Auth UI placeholder (DEC-025 §3, REQ-UI-006).
 *
 * Per DEC-025: admin-ui does NOT host a login page. Auth is delegated to the
 * surrounding reverse proxy / OIDC integration. When the bearer token expires
 * (401), the SPA redirects to `VITE_LOGIN_URL` for the proxy to re-authenticate.
 *
 * If `VITE_LOGIN_URL` is empty (dev deployments, mis-configured prod), the SPA
 * never silently hangs: it surfaces a "session expired; reload" toast so the
 * operator can manually re-auth. (Per sprint-10b reviewer ask #4.)
 *
 * For 403, this handler is a no-op — `<RequireScope>` already hides actions
 * the operator can't perform. A 403 that reaches the catch path means the
 * backend rejected an action that the scope hint failed to predict (e.g.
 * scope endpoint absent on older deploys). In that case the *pane* decides
 * to render `<ForbiddenPane>` inline.
 */

import { ApiError } from '@xiaoguai/shared';

/**
 * What `handleAuthError` decided to do. The caller (typically a
 * top-level effect listening for unhandled promise rejections) acts on it.
 */
export type AuthAction =
  | { kind: 'redirect'; url: string }
  | { kind: 'toast'; messageKey: 'auth.sessionExpired' }
  | { kind: 'ignore' };

/**
 * Decide what to do for a given error. Pure — does not touch `window`.
 *
 * @param err arbitrary thrown value (we narrow to `ApiError`)
 * @param loginUrl `import.meta.env.VITE_LOGIN_URL` value (or empty string)
 */
export function decideAuthAction(err: unknown, loginUrl: string | undefined): AuthAction {
  if (!(err instanceof ApiError)) return { kind: 'ignore' };
  if (err.status === 401) {
    if (loginUrl && loginUrl.length > 0) {
      return { kind: 'redirect', url: loginUrl };
    }
    return { kind: 'toast', messageKey: 'auth.sessionExpired' };
  }
  // 403 is RequireScope / ForbiddenPane territory, not global handler's job.
  return { kind: 'ignore' };
}

/**
 * Perform the side effect from `decideAuthAction`. Separated so unit tests can
 * exercise the pure decision logic without mocking `window.location`.
 */
export function performAuthAction(
  action: AuthAction,
  effects: {
    redirect: (url: string) => void;
    toast: (key: string) => void;
  },
): void {
  switch (action.kind) {
    case 'redirect':
      effects.redirect(action.url);
      break;
    case 'toast':
      effects.toast(action.messageKey);
      break;
    case 'ignore':
      break;
  }
}
