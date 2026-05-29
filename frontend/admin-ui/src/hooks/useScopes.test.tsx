/**
 * v1.8.0 (sprint-10b S10b-6) — tests for ScopeProvider + useScopes.
 *
 * Covers:
 *   * Happy path: provider loads scopes from a mock client, hasScope()
 *     returns true / false correctly.
 *   * Fail-open: when listMyScopes() throws ApiError(404), hasScope()
 *     returns true for every scope.
 *   * Network error path: same as 404 (fail-open).
 *   * 403 path: scopes remain empty, hasScope() returns false.
 *   * useScopes() throws when called outside a provider.
 */

import { describe, expect, it, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { ApiError, type MyScopesResponse } from '@xiaoguai/shared';
import {
  ScopeProvider,
  useScopes,
  __resetFailOpenWarned,
} from './useScopes';

function makeClient(impl: () => Promise<MyScopesResponse>) {
  return { listMyScopes: impl };
}

function ScopeInspector({ probe }: { probe: string }): JSX.Element {
  const { ready, failOpen, hasScope } = useScopes();
  return (
    <div>
      <span data-testid="ready">{ready ? 'yes' : 'no'}</span>
      <span data-testid="failOpen">{failOpen ? 'yes' : 'no'}</span>
      <span data-testid="has">{hasScope(probe) ? 'yes' : 'no'}</span>
    </div>
  );
}

beforeEach(() => {
  __resetFailOpenWarned();
});

describe('ScopeProvider', () => {
  it('exposes the resolved scope set after the fetch resolves', async () => {
    const client = makeClient(async () => ({
      scopes: ['personas.read', 'personas.write'],
    }));
    render(
      <ScopeProvider client={client}>
        <ScopeInspector probe="personas.write" />
      </ScopeProvider>,
    );
    await waitFor(() =>
      expect(screen.getByTestId('ready').textContent).toBe('yes'),
    );
    expect(screen.getByTestId('failOpen').textContent).toBe('no');
    expect(screen.getByTestId('has').textContent).toBe('yes');
  });

  it('returns false for scopes that are absent from the response', async () => {
    const client = makeClient(async () => ({
      scopes: ['personas.read'],
    }));
    render(
      <ScopeProvider client={client}>
        <ScopeInspector probe="personas.write" />
      </ScopeProvider>,
    );
    await waitFor(() =>
      expect(screen.getByTestId('ready').textContent).toBe('yes'),
    );
    expect(screen.getByTestId('has').textContent).toBe('no');
  });

  it('fails OPEN on 404 (older backend without /me/scopes)', async () => {
    const client = makeClient(async () => {
      throw new ApiError(404, 'not_found', 'no such route');
    });
    render(
      <ScopeProvider client={client}>
        <ScopeInspector probe="something.write" />
      </ScopeProvider>,
    );
    await waitFor(() =>
      expect(screen.getByTestId('ready').textContent).toBe('yes'),
    );
    expect(screen.getByTestId('failOpen').textContent).toBe('yes');
    expect(screen.getByTestId('has').textContent).toBe('yes');
  });

  it('fails OPEN on network errors (TypeError / fetch failure)', async () => {
    const client = makeClient(async () => {
      throw new TypeError('network down');
    });
    render(
      <ScopeProvider client={client}>
        <ScopeInspector probe="audit.export" />
      </ScopeProvider>,
    );
    await waitFor(() =>
      expect(screen.getByTestId('ready').textContent).toBe('yes'),
    );
    expect(screen.getByTestId('failOpen').textContent).toBe('yes');
    expect(screen.getByTestId('has').textContent).toBe('yes');
  });

  it('fails CLOSED on 403 (auth banner handles the rest)', async () => {
    const client = makeClient(async () => {
      throw new ApiError(403, 'forbidden', 'no scope');
    });
    render(
      <ScopeProvider client={client}>
        <ScopeInspector probe="personas.write" />
      </ScopeProvider>,
    );
    await waitFor(() =>
      expect(screen.getByTestId('ready').textContent).toBe('yes'),
    );
    expect(screen.getByTestId('failOpen').textContent).toBe('no');
    expect(screen.getByTestId('has').textContent).toBe('no');
  });
});

describe('useScopes outside a provider', () => {
  it('throws a programmer-error message', () => {
    function Naked() {
      useScopes();
      return null;
    }
    // React logs the error to console.error; swallow it for cleanliness.
    const origErr = console.error;
    console.error = () => {};
    try {
      expect(() => render(<Naked />)).toThrow(/<ScopeProvider>/);
    } finally {
      console.error = origErr;
    }
  });
});
