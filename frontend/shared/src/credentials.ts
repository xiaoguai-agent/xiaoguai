/**
 * In-memory single-owner credential store (SEC-16).
 *
 * DEC-033: the backend is gated (if at all) by one username + password via
 * HTTP Basic. These credentials previously lived in `sessionStorage`
 * (`xiaoguai.basic.password`), where any XSS could exfiltrate the instance's
 * only credential. They now live solely in this module-scoped variable:
 * nothing is written to sessionStorage / localStorage / cookies.
 *
 * Trade-off (owner-approved): a page refresh drops the credentials and the
 * UI re-prompts for login on the first 401. The UIs may still keep the
 * *username* (not a secret) in sessionStorage for login-form prefill, but
 * the password is never persisted anywhere.
 */

/** Single-owner HTTP Basic credentials (DEC-033). */
export interface BasicCredentials {
  username: string;
  password: string;
}

let current: BasicCredentials | undefined;

/** Hold the owner credentials in memory. Never persisted (SEC-16). */
export function setBasicCredentials(username: string, password: string): void {
  current = { username, password };
}

/** Drop the in-memory credentials (sign-out / failed login). */
export function clearBasicCredentials(): void {
  current = undefined;
}

/**
 * Snapshot of the held credentials, or `undefined` when signed out.
 * Returns a copy so callers cannot mutate the stored object.
 */
export function getBasicCredentials(): BasicCredentials | undefined {
  return current === undefined ? undefined : { ...current };
}

/** True when owner credentials are currently held in memory. */
export function hasBasicCredentials(): boolean {
  return current !== undefined;
}
