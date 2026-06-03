/**
 * AuthGate — single-owner HTTP Basic login (DEC-033).
 *
 * The backend has no OIDC/JWT/tenants; access (when the owner has configured
 * one) is a single username + password via HTTP Basic. This gate registers a
 * 401 handler on the shared client so that whenever any request comes back
 * `401 Unauthorized` — i.e. the backend has a credential set and we don't have
 * it (or it's wrong) — a login modal appears. When the backend runs open (no
 * credential), nothing ever 401s and this is invisible.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import { ApiError } from '@xiaoguai/shared';
import { client, setCredentials, clearCredentials } from '../client';

export function AuthGate({ children }: { children: React.ReactNode }) {
  const [open, setOpen] = useState(false);
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  // Avoid re-opening churn while the modal is already up.
  const openRef = useRef(false);
  openRef.current = open;

  useEffect(() => {
    client.setUnauthorizedHandler(() => {
      if (!openRef.current) setOpen(true);
    });
    return () => client.setUnauthorizedHandler(undefined);
  }, []);

  const onSubmit = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault();
      if (submitting) return;
      setSubmitting(true);
      setError(null);
      setCredentials(username, password);
      try {
        // Probe a gated endpoint to validate the credentials before reloading.
        // `/v1/mcp/servers` sits behind the owner-auth layer: wrong creds →
        // 401, right creds → 200/503 (both resolve without throwing 401).
        await client.listMcpServers();
        // Success — reload so any data that failed while unauthenticated refetches.
        window.location.reload();
      } catch (err) {
        if (err instanceof ApiError && err.status === 401) {
          clearCredentials();
          setError('Incorrect username or password.');
        } else {
          // Non-auth error (e.g. network) — credentials are likely fine; close.
          setOpen(false);
        }
      } finally {
        setSubmitting(false);
      }
    },
    [username, password, submitting],
  );

  return (
    <>
      {children}
      {open && (
        <div className="auth-overlay" role="dialog" aria-modal="true" aria-label="Sign in">
          <form className="auth-modal" onSubmit={onSubmit}>
            <h2 className="auth-modal__title">Sign in</h2>
            <p className="auth-modal__hint">
              This Xiaoguai instance requires the owner username and password.
            </p>
            <label className="auth-modal__field">
              <span>Username</span>
              <input
                type="text"
                autoComplete="username"
                value={username}
                onChange={(ev) => setUsername(ev.target.value)}
                autoFocus
              />
            </label>
            <label className="auth-modal__field">
              <span>Password</span>
              <input
                type="password"
                autoComplete="current-password"
                value={password}
                onChange={(ev) => setPassword(ev.target.value)}
              />
            </label>
            {error && (
              <div className="auth-modal__error" role="alert">
                {error}
              </div>
            )}
            <button
              type="submit"
              className="auth-modal__submit"
              disabled={submitting || !username || !password}
            >
              {submitting ? 'Signing in…' : 'Sign in'}
            </button>
          </form>
        </div>
      )}
    </>
  );
}
