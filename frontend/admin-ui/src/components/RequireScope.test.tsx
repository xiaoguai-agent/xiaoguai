/**
 * v1.8.0 (sprint-10b S10b-6) — tests for <RequireScope>.
 *
 * Renders the gate inside a real <ScopeProvider> backed by a mock
 * client so the full ready / hasScope / fallback path is exercised.
 */

import { describe, expect, it, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { ApiError, type MyScopesResponse } from '@xiaoguai/shared';
import { ScopeProvider, __resetFailOpenWarned } from '../hooks/useScopes';
import { RequireScope } from './RequireScope';

function makeClient(impl: () => Promise<MyScopesResponse>) {
  return { listMyScopes: impl };
}

beforeEach(() => {
  __resetFailOpenWarned();
});

describe('<RequireScope>', () => {
  it('renders children when the scope is granted', async () => {
    const client = makeClient(async () => ({ scopes: ['personas.write'] }));
    render(
      <ScopeProvider client={client}>
        <RequireScope name="personas.write">
          <button>save</button>
        </RequireScope>
      </ScopeProvider>,
    );
    await waitFor(() => expect(screen.getByText('save')).toBeTruthy());
  });

  it('renders fallback when the scope is not granted', async () => {
    const client = makeClient(async () => ({ scopes: ['personas.read'] }));
    render(
      <ScopeProvider client={client}>
        <RequireScope
          name="personas.write"
          fallback={<span>not-allowed</span>}
        >
          <button>save</button>
        </RequireScope>
      </ScopeProvider>,
    );
    await waitFor(() => expect(screen.getByText('not-allowed')).toBeTruthy());
    expect(screen.queryByText('save')).toBeNull();
  });

  it('hides children by default (null fallback) when scope is absent', async () => {
    const client = makeClient(async () => ({ scopes: [] }));
    render(
      <ScopeProvider client={client}>
        <RequireScope name="personas.write">
          <button>save</button>
        </RequireScope>
      </ScopeProvider>,
    );
    // Wait for the provider to flip ready.
    await waitFor(() => expect(screen.queryByText('save')).toBeNull());
  });

  it('renders children when /me/scopes returned 404 (fail-open)', async () => {
    const client = makeClient(async () => {
      throw new ApiError(404, 'not_found', 'no such route');
    });
    render(
      <ScopeProvider client={client}>
        <RequireScope name="personas.write">
          <button>save</button>
        </RequireScope>
      </ScopeProvider>,
    );
    await waitFor(() => expect(screen.getByText('save')).toBeTruthy());
  });

  it('renders fallback while the initial fetch is in flight', () => {
    // Resolve never — provider stays not-ready.
    const client = makeClient(() => new Promise<MyScopesResponse>(() => {}));
    render(
      <ScopeProvider client={client}>
        <RequireScope name="personas.write" fallback={<span>loading</span>}>
          <button>save</button>
        </RequireScope>
      </ScopeProvider>,
    );
    expect(screen.getByText('loading')).toBeTruthy();
    expect(screen.queryByText('save')).toBeNull();
  });
});
