import { XiaoguaiClient, type BasicCredentials } from '@xiaoguai/shared';

const baseUrl =
  (import.meta.env.VITE_API_URL as string | undefined) ??
  (typeof window !== 'undefined' ? window.location.origin : 'http://localhost:8080');

// DEC-033 single-owner auth: the backend is gated (if at all) by one
// username + password via HTTP Basic. When the owner hasn't configured a
// credential the backend runs open and we send no Authorization header.
// Credentials live in sessionStorage so they survive reloads within a tab
// but are not persisted to disk.
const USER_KEY = 'xiaoguai.basic.username';
const PASS_KEY = 'xiaoguai.basic.password';

function loadStoredCredentials(): BasicCredentials | undefined {
  try {
    const username = sessionStorage.getItem(USER_KEY);
    const password = sessionStorage.getItem(PASS_KEY);
    if (username && password) return { username, password };
  } catch {
    // sessionStorage unavailable (private mode) — run unauthenticated.
  }
  return undefined;
}

export const client = new XiaoguaiClient({
  baseUrl,
  basicAuth: loadStoredCredentials(),
});

/** True when owner credentials are currently held. */
export function hasCredentials(): boolean {
  return loadStoredCredentials() !== undefined;
}

/** Store owner credentials and apply them to the live client. */
export function setCredentials(username: string, password: string): void {
  try {
    sessionStorage.setItem(USER_KEY, username);
    sessionStorage.setItem(PASS_KEY, password);
  } catch {
    // sessionStorage unavailable — apply in-memory only for this session.
  }
  client.setBasicAuth({ username, password });
}

/** Forget owner credentials (sign out). */
export function clearCredentials(): void {
  try {
    sessionStorage.removeItem(USER_KEY);
    sessionStorage.removeItem(PASS_KEY);
  } catch {
    // ignore
  }
  client.setBasicAuth(undefined);
}
