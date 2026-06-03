/**
 * Tests for ScopeProvider + useScopes.
 *
 * Under the single-user pivot (DEC-033) there is one owner who has every
 * scope, and `/v1/admin/me/scopes` is gone. The provider resolves
 * immediately into a fail-open state: ready=true, failOpen=true, and
 * hasScope() returns true for everything. No fetch, no client.
 *
 * Covers:
 *   * Provider is immediately ready + fail-open.
 *   * hasScope() returns true for any scope (and is unaffected by a
 *     `client` prop, which is now ignored).
 *   * useScopes() throws when called outside a provider.
 */

import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { ScopeProvider, useScopes } from './useScopes';

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

describe('ScopeProvider', () => {
  it('is immediately ready and fails open (single owner has every scope)', () => {
    render(
      <ScopeProvider>
        <ScopeInspector probe="personas.write" />
      </ScopeProvider>,
    );
    expect(screen.getByTestId('ready').textContent).toBe('yes');
    expect(screen.getByTestId('failOpen').textContent).toBe('yes');
    expect(screen.getByTestId('has').textContent).toBe('yes');
  });

  it('grants any scope regardless of name', () => {
    render(
      <ScopeProvider>
        <ScopeInspector probe="audit.export" />
      </ScopeProvider>,
    );
    expect(screen.getByTestId('has').textContent).toBe('yes');
  });

  it('ignores a provided client prop (retained only for source compat)', () => {
    render(
      <ScopeProvider client={{ listMyScopes: async () => ({ scopes: [] }) }}>
        <ScopeInspector probe="something.write" />
      </ScopeProvider>,
    );
    expect(screen.getByTestId('ready').textContent).toBe('yes');
    expect(screen.getByTestId('has').textContent).toBe('yes');
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
