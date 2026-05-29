/**
 * Tests for the auth error decision logic (sprint-10b S10b-9).
 */
import { describe, expect, it, vi } from 'vitest';
import { ApiError } from '@xiaoguai/shared';
import { decideAuthAction, performAuthAction } from './handleAuthError';

describe('decideAuthAction', () => {
  it('returns redirect when 401 + VITE_LOGIN_URL set', () => {
    const err = new ApiError(401, 'unauthorized', 'token expired');
    expect(decideAuthAction(err, 'https://login.example.com')).toEqual({
      kind: 'redirect',
      url: 'https://login.example.com',
    });
  });

  it('returns toast when 401 + VITE_LOGIN_URL empty', () => {
    const err = new ApiError(401, 'unauthorized', 'token expired');
    expect(decideAuthAction(err, '')).toEqual({
      kind: 'toast',
      messageKey: 'auth.sessionExpired',
    });
  });

  it('returns toast when 401 + VITE_LOGIN_URL undefined', () => {
    const err = new ApiError(401, 'unauthorized', 'token expired');
    expect(decideAuthAction(err, undefined)).toEqual({
      kind: 'toast',
      messageKey: 'auth.sessionExpired',
    });
  });

  it('returns ignore for 403 — RequireScope handles it', () => {
    const err = new ApiError(403, 'forbidden', 'scope absent');
    expect(decideAuthAction(err, 'https://login.example.com')).toEqual({
      kind: 'ignore',
    });
  });

  it('returns ignore for 404 — not an auth concern', () => {
    const err = new ApiError(404, 'not_found', 'unknown route');
    expect(decideAuthAction(err, '')).toEqual({ kind: 'ignore' });
  });

  it('returns ignore for 5xx — not an auth concern', () => {
    const err = new ApiError(500, 'server', 'internal');
    expect(decideAuthAction(err, '')).toEqual({ kind: 'ignore' });
  });

  it('returns ignore for non-ApiError values', () => {
    expect(decideAuthAction(new Error('plain'), 'https://x')).toEqual({ kind: 'ignore' });
    expect(decideAuthAction('plain string', 'https://x')).toEqual({ kind: 'ignore' });
    expect(decideAuthAction(null, 'https://x')).toEqual({ kind: 'ignore' });
    expect(decideAuthAction(undefined, 'https://x')).toEqual({ kind: 'ignore' });
  });
});

describe('performAuthAction', () => {
  it('calls redirect for { kind: redirect }', () => {
    const redirect = vi.fn();
    const toast = vi.fn();
    performAuthAction({ kind: 'redirect', url: 'https://x' }, { redirect, toast });
    expect(redirect).toHaveBeenCalledWith('https://x');
    expect(toast).not.toHaveBeenCalled();
  });

  it('calls toast for { kind: toast }', () => {
    const redirect = vi.fn();
    const toast = vi.fn();
    performAuthAction(
      { kind: 'toast', messageKey: 'auth.sessionExpired' },
      { redirect, toast },
    );
    expect(toast).toHaveBeenCalledWith('auth.sessionExpired');
    expect(redirect).not.toHaveBeenCalled();
  });

  it('does nothing for { kind: ignore }', () => {
    const redirect = vi.fn();
    const toast = vi.fn();
    performAuthAction({ kind: 'ignore' }, { redirect, toast });
    expect(redirect).not.toHaveBeenCalled();
    expect(toast).not.toHaveBeenCalled();
  });
});
