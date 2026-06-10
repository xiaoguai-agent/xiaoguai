/**
 * In-memory credential store (SEC-16) — credentials must round-trip through
 * the module variable, never touch Web Storage, and be cleared on sign-out.
 */

import { afterEach, describe, expect, it } from 'vitest';
import {
  clearBasicCredentials,
  getBasicCredentials,
  hasBasicCredentials,
  setBasicCredentials,
} from './credentials';

afterEach(() => {
  clearBasicCredentials();
});

describe('basic credential store', () => {
  it('starts empty', () => {
    expect(hasBasicCredentials()).toBe(false);
    expect(getBasicCredentials()).toBeUndefined();
  });

  it('round-trips set → get → clear', () => {
    setBasicCredentials('owner', 's3cret');
    expect(hasBasicCredentials()).toBe(true);
    expect(getBasicCredentials()).toEqual({ username: 'owner', password: 's3cret' });

    clearBasicCredentials();
    expect(hasBasicCredentials()).toBe(false);
    expect(getBasicCredentials()).toBeUndefined();
  });

  it('returns a copy — callers cannot mutate the stored credentials', () => {
    setBasicCredentials('owner', 's3cret');
    const snapshot = getBasicCredentials();
    expect(snapshot).toBeDefined();
    if (snapshot) snapshot.password = 'tampered';
    expect(getBasicCredentials()).toEqual({ username: 'owner', password: 's3cret' });
  });
});
