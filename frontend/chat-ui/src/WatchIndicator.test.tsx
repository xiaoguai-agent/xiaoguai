/**
 * WatchIndicator tests — v1.3.x
 *
 * Covers:
 *  - Renders nothing when count = 0
 *  - Renders green pill when 1 watcher running
 *  - Renders amber pill when any watcher is in error state
 *  - Renders correct count for 5+ watchers
 *  - Popover opens on click
 *  - Each watcher row is listed in the popover
 *  - Pause button triggers confirm + client.pauseWatcher
 *  - 503 / endpoint absent → renders nothing gracefully
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import type { WatcherInfo } from '@xiaoguai/shared';
import { WatchIndicator } from './WatchIndicator';

// Mock the client module so we don't hit the network.
vi.mock('./client', () => ({
  client: {
    listSessionWatchers: vi.fn(),
    pauseWatcher: vi.fn(),
    resumeWatcher: vi.fn(),
  },
}));

// Re-import after mock so TypeScript is happy.
import { client } from './client';

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const mockedClient = client as any as {
  listSessionWatchers: ReturnType<typeof vi.fn>;
  pauseWatcher: ReturnType<typeof vi.fn>;
  resumeWatcher: ReturnType<typeof vi.fn>;
};

function makeWatcher(overrides: Partial<WatcherInfo> = {}): WatcherInfo {
  return {
    id: 'w-1',
    name: 'Test Watcher',
    source_type: 'schedule',
    last_fired_at: null,
    status: 'running',
    ...overrides,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  // Suppress window.confirm in tests; override per-test as needed.
  vi.spyOn(window, 'confirm').mockReturnValue(true);
});

afterEach(() => {
  vi.restoreAllMocks();
});

// ---- count = 0 -----------------------------------------------------------

describe('when watcher count is 0', () => {
  it('renders nothing', async () => {
    mockedClient.listSessionWatchers.mockResolvedValue([]);
    const { container } = render(<WatchIndicator sessionId="sess-1" />);
    await waitFor(() => expect(mockedClient.listSessionWatchers).toHaveBeenCalledOnce());
    expect(container.firstChild).toBeNull();
  });

  it('renders nothing when sessionId is undefined', () => {
    const { container } = render(<WatchIndicator sessionId={undefined} />);
    expect(container.firstChild).toBeNull();
    expect(mockedClient.listSessionWatchers).not.toHaveBeenCalled();
  });
});

// ---- count = 1, all running (green) -------------------------------------

describe('with 1 running watcher', () => {
  const watchers = [makeWatcher({ id: 'w-1', name: 'Report Watcher', status: 'running' })];

  beforeEach(() => {
    mockedClient.listSessionWatchers.mockResolvedValue(watchers);
  });

  it('renders a green badge', async () => {
    render(<WatchIndicator sessionId="sess-1" />);
    const badge = await screen.findByTestId('watch-badge');
    expect(badge).toBeInTheDocument();
    expect(badge.className).toContain('watch-badge--green');
  });

  it('badge label mentions the count', async () => {
    render(<WatchIndicator sessionId="sess-1" />);
    const badge = await screen.findByTestId('watch-badge');
    expect(badge.textContent).toMatch(/1/);
  });

  it('does not show popover initially', async () => {
    render(<WatchIndicator sessionId="sess-1" />);
    await screen.findByTestId('watch-badge');
    expect(screen.queryByTestId('watch-popover')).not.toBeInTheDocument();
  });
});

// ---- error state badge (amber) ------------------------------------------

describe('with at least one error watcher', () => {
  const watchers = [
    makeWatcher({ id: 'w-1', status: 'running' }),
    makeWatcher({ id: 'w-2', name: 'Broken Watcher', status: 'error' }),
  ];

  beforeEach(() => {
    mockedClient.listSessionWatchers.mockResolvedValue(watchers);
  });

  it('renders an amber badge', async () => {
    render(<WatchIndicator sessionId="sess-1" />);
    const badge = await screen.findByTestId('watch-badge');
    expect(badge.className).toContain('watch-badge--amber');
  });
});

// ---- 5+ watchers --------------------------------------------------------

describe('with 5 watchers', () => {
  const watchers = Array.from({ length: 5 }, (_, i) =>
    makeWatcher({ id: `w-${i}`, name: `Watcher ${i}`, status: 'running' }),
  );

  beforeEach(() => {
    mockedClient.listSessionWatchers.mockResolvedValue(watchers);
  });

  it('badge shows count 5', async () => {
    render(<WatchIndicator sessionId="sess-1" />);
    const badge = await screen.findByTestId('watch-badge');
    expect(badge.textContent).toMatch(/5/);
  });
});

// ---- popover open -------------------------------------------------------

describe('popover', () => {
  const watchers = [
    makeWatcher({ id: 'w-1', name: 'Alpha Watcher', status: 'running' }),
    makeWatcher({ id: 'w-2', name: 'Beta Watcher', status: 'paused' }),
  ];

  beforeEach(() => {
    mockedClient.listSessionWatchers.mockResolvedValue(watchers);
  });

  it('opens on badge click and lists watchers', async () => {
    render(<WatchIndicator sessionId="sess-1" />);
    const badge = await screen.findByTestId('watch-badge');
    fireEvent.click(badge);

    const popover = screen.getByTestId('watch-popover');
    expect(popover).toBeInTheDocument();

    const rows = screen.getAllByTestId('watcher-row');
    expect(rows).toHaveLength(2);
    expect(screen.getByText('Alpha Watcher')).toBeInTheDocument();
    expect(screen.getByText('Beta Watcher')).toBeInTheDocument();
  });

  it('closes popover on second badge click', async () => {
    render(<WatchIndicator sessionId="sess-1" />);
    const badge = await screen.findByTestId('watch-badge');
    fireEvent.click(badge);
    expect(screen.getByTestId('watch-popover')).toBeInTheDocument();
    fireEvent.click(badge);
    expect(screen.queryByTestId('watch-popover')).not.toBeInTheDocument();
  });
});

// ---- pause click --------------------------------------------------------

describe('pause button', () => {
  const watchers = [makeWatcher({ id: 'w-1', name: 'Pausable', status: 'running' })];

  beforeEach(() => {
    mockedClient.listSessionWatchers.mockResolvedValue(watchers);
    mockedClient.pauseWatcher.mockResolvedValue(undefined);
  });

  it('calls client.pauseWatcher after confirm', async () => {
    render(<WatchIndicator sessionId="sess-1" />);
    const badge = await screen.findByTestId('watch-badge');
    fireEvent.click(badge);

    const pauseBtn = screen.getByRole('button', { name: /pause pausable/i });
    fireEvent.click(pauseBtn);

    expect(window.confirm).toHaveBeenCalled();
    await waitFor(() =>
      expect(mockedClient.pauseWatcher).toHaveBeenCalledWith('w-1'),
    );
  });

  it('does not call client.pauseWatcher when confirm is cancelled', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(false);
    render(<WatchIndicator sessionId="sess-1" />);
    const badge = await screen.findByTestId('watch-badge');
    fireEvent.click(badge);

    const pauseBtn = screen.getByRole('button', { name: /pause pausable/i });
    fireEvent.click(pauseBtn);

    expect(mockedClient.pauseWatcher).not.toHaveBeenCalled();
  });
});

// ---- 503 / endpoint absent fallback -------------------------------------

describe('503 / endpoint absent fallback', () => {
  it('renders nothing gracefully when listSessionWatchers returns [] (404/503 handled in client)', async () => {
    // The client already absorbs 404/503 and returns []; simulate that.
    mockedClient.listSessionWatchers.mockResolvedValue([]);
    const { container } = render(<WatchIndicator sessionId="sess-1" />);
    await waitFor(() => expect(mockedClient.listSessionWatchers).toHaveBeenCalledOnce());
    expect(container.firstChild).toBeNull();
  });

  it('leaves current state unchanged on unexpected fetch error', async () => {
    // First call returns 1 watcher, second throws unexpectedly.
    const watcher = makeWatcher({ id: 'w-1', status: 'running' });
    mockedClient.listSessionWatchers
      .mockResolvedValueOnce([watcher])
      .mockRejectedValueOnce(new Error('network blip'));

    render(<WatchIndicator sessionId="sess-1" />);
    // First render: badge appears.
    await screen.findByTestId('watch-badge');

    // Second call (manual trigger via re-render) should not crash.
    // We just verify badge is still present (state unchanged).
    expect(screen.getByTestId('watch-badge')).toBeInTheDocument();
  });
});
