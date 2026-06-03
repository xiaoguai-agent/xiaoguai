/**
 * Tests for <RequireScope>.
 *
 * Under the single-user pivot (DEC-033) the owner holds every scope and
 * `<ScopeProvider>` fails open immediately (ready=true, hasScope=true).
 * The gate therefore always renders its children. The fallback path only
 * remains reachable in principle if a future provider reports not-ready;
 * with the current provider it never does.
 */

import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { ScopeProvider } from '../hooks/useScopes';
import { RequireScope } from './RequireScope';

describe('<RequireScope>', () => {
  it('renders children (single owner holds every scope)', () => {
    render(
      <ScopeProvider>
        <RequireScope name="personas.write">
          <button>save</button>
        </RequireScope>
      </ScopeProvider>,
    );
    expect(screen.getByText('save')).toBeTruthy();
  });

  it('renders children for any scope name, ignoring the fallback', () => {
    render(
      <ScopeProvider>
        <RequireScope
          name="audit.export"
          fallback={<span>not-allowed</span>}
        >
          <button>export</button>
        </RequireScope>
      </ScopeProvider>,
    );
    expect(screen.getByText('export')).toBeTruthy();
    expect(screen.queryByText('not-allowed')).toBeNull();
  });
});
