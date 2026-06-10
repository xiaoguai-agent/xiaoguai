import {
  XiaoguaiClient,
  clearBasicCredentials,
  getBasicCredentials,
  hasBasicCredentials,
  setBasicCredentials,
} from '@xiaoguai/shared';

const baseUrl =
  (import.meta.env.VITE_API_URL as string | undefined) ??
  (typeof window !== 'undefined' ? window.location.origin : 'http://localhost:7600');

// DEC-033 single-owner auth: the backend is gated (if at all) by one
// username + password via HTTP Basic. When the owner hasn't configured a
// credential the backend runs open and we send no Authorization header.
//
// SEC-16: the password lives only in memory (module variable inside
// @xiaoguai/shared), never in sessionStorage/localStorage where any XSS
// could read the instance's only credential. Trade-off (owner-approved):
// a page refresh drops it and AuthGate re-prompts on the first 401. Only
// the username — not a secret — stays in sessionStorage so the login form
// can prefill it.
const USER_KEY = 'xiaoguai.basic.username';
/** Key under which older builds persisted the password (SEC-16). */
const LEGACY_PASS_KEY = 'xiaoguai.basic.password';

// SEC-16: purge any password persisted by a previous build of this UI.
try {
  sessionStorage.removeItem(LEGACY_PASS_KEY);
} catch {
  // sessionStorage unavailable (private mode / SSR) — nothing persisted.
}

export const client = new XiaoguaiClient({
  baseUrl,
  // SEC-16: every page load starts signed out; AuthGate prompts on 401.
  basicAuth: getBasicCredentials(),
});

/** True when owner credentials are currently held (in memory only). */
export function hasCredentials(): boolean {
  return hasBasicCredentials();
}

/** Username from the last sign-in in this tab, for login-form prefill. */
export function lastUsername(): string {
  try {
    return sessionStorage.getItem(USER_KEY) ?? '';
  } catch {
    return '';
  }
}

/** Hold owner credentials in memory and apply them to the live client. */
export function setCredentials(username: string, password: string): void {
  // SEC-16: memory only — the password is never written to Web Storage.
  setBasicCredentials(username, password);
  client.setBasicAuth(getBasicCredentials());
  try {
    // Prefill convenience only; the username is not a secret.
    sessionStorage.setItem(USER_KEY, username);
  } catch {
    // sessionStorage unavailable — prefill is best-effort.
  }
}

/** Forget owner credentials (sign out). */
export function clearCredentials(): void {
  clearBasicCredentials();
  client.setBasicAuth(undefined);
}
