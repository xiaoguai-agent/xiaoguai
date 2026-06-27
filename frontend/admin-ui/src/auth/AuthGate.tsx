/**
 * AuthGate — single-owner HTTP Basic login (DEC-033).
 *
 * The backend has no OIDC/JWT and no multi-owner model; access (when the
 * owner has configured one) is a single username + password via HTTP Basic.
 * This gate registers a
 * 401 handler on the shared client so that whenever any request returns
 * `401 Unauthorized` a login modal appears. When the backend runs open (no
 * credential), nothing ever 401s and this is invisible.
 *
 * Supersedes the old redirect-to-VITE_LOGIN_URL behaviour (DEC-025), which
 * assumed an OIDC reverse proxy that the single-user pivot removed.
 *
 * SEC-16: credentials are memory-only (never Web Storage), so every page
 * refresh starts signed out and the first 401 re-opens this modal. Only the
 * username (not a secret) is kept in sessionStorage for prefill.
 */

import { Fragment, useCallback, useEffect, useRef, useState } from 'react';
import { ApiError } from '@xiaoguai/shared';
import { client, setCredentials, clearCredentials, lastUsername } from '../client';

export function AuthGate({ children }: { children: React.ReactNode }) {
  const [open, setOpen] = useState(false);
  // SEC-16: prefill the username from the last sign-in in this tab.
  const [username, setUsername] = useState(lastUsername);
  const [password, setPassword] = useState('');
  // Bumped on successful login to remount (and thus refetch) the app subtree.
  const [epoch, setEpoch] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
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
        // Success. SEC-16: credentials are memory-only — a full reload would
        // drop them and loop back to this modal. Remount the app subtree
        // instead so everything that failed while unauthenticated refetches.
        setPassword('');
        setOpen(false);
        setEpoch((n) => n + 1);
      } catch (err) {
        if (err instanceof ApiError && err.status === 401) {
          clearCredentials();
          setError('Incorrect username or password.');
        } else {
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
      {/* Keyed so a successful login remounts the subtree (refetch-all). */}
      <Fragment key={epoch}>{children}</Fragment>
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
