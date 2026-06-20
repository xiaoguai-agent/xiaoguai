/**
 * DEC-041 (frontend half) — tests for the shared useAsyncState hook, driven
 * through a tiny harness component (realistic usage, no renderHook coupling).
 */
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useAsyncState } from './useAsyncState';

function Harness({ loader }: { loader: () => Promise<string> }) {
  const { data, error, loading, reload } = useAsyncState(loader, []);
  return (
    <div>
      <span data-testid="loading">{String(loading)}</span>
      <span data-testid="data">{data ?? ''}</span>
      <span data-testid="error">{error ?? ''}</span>
      <button type="button" onClick={reload}>
        reload
      </button>
    </div>
  );
}

describe('useAsyncState', () => {
  it('starts loading, then exposes resolved data', async () => {
    render(<Harness loader={() => Promise.resolve('hello')} />);
    expect(screen.getByTestId('loading')).toHaveTextContent('true');
    await waitFor(() => expect(screen.getByTestId('data')).toHaveTextContent('hello'));
    expect(screen.getByTestId('loading')).toHaveTextContent('false');
    expect(screen.getByTestId('error')).toHaveTextContent('');
  });

  it('captures the error message on rejection', async () => {
    render(<Harness loader={() => Promise.reject(new Error('nope'))} />);
    await waitFor(() => expect(screen.getByTestId('error')).toHaveTextContent('nope'));
    expect(screen.getByTestId('loading')).toHaveTextContent('false');
    expect(screen.getByTestId('data')).toHaveTextContent('');
  });

  it('reload() re-invokes the loader', async () => {
    const loader = vi.fn(() => Promise.resolve('v'));
    render(<Harness loader={loader} />);
    await waitFor(() => expect(loader).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByText('reload'));
    await waitFor(() => expect(loader).toHaveBeenCalledTimes(2));
  });
});
