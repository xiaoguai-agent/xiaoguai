/**
 * RecentOutcomesPanel — unit tests.
 *
 * Covers:
 *  - Returns null when no sessionId is provided.
 *  - Shows "Loading…" immediately on first render with a session.
 *  - Renders kind chips and recent events once the API resolves.
 *  - Shows an error message when the API rejects.
 *  - Renders "No outcomes yet." when by_kind is empty.
 *  - Audit link encodes session_id correctly.
 *  - Calls getSessionOutcomesSummary with the correct session id.
 */

import { render, screen, waitFor } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock the client module before importing the component.
vi.mock('./client', () => ({
  client: {
    getSessionOutcomesSummary: vi.fn(),
  },
}));

import { client } from './client';
import { RecentOutcomesPanel } from './RecentOutcomesPanel';
import type { SessionOutcomesSummary } from '@xiaoguai/shared';

const mockClient = client as unknown as { getSessionOutcomesSummary: ReturnType<typeof vi.fn> };

const sampleSummary: SessionOutcomesSummary = {
  session_id: 'ses-001',
  tenant_id: 'ten_dev',
  by_kind: {
    tickets_resolved: { count: 3, sum: 3, unit: null },
    hours_saved: { count: 1, sum: 2.5, unit: 'h' },
  },
  recent: [
    {
      kind: 'tickets_resolved',
      value: 1,
      unit: null,
      description: 'Ticket #42 resolved',
      ts: '2026-05-25T10:00:00Z',
    },
    {
      kind: 'hours_saved',
      value: 2.5,
      unit: 'h',
      description: null,
      ts: '2026-05-25T09:45:00Z',
    },
  ],
};

beforeEach(() => {
  vi.clearAllMocks();
});

describe('RecentOutcomesPanel', () => {
  it('renders nothing when sessionId is undefined', () => {
    const { container } = render(<RecentOutcomesPanel sessionId={undefined} />);
    expect(container.firstChild).toBeNull();
  });

  it('shows Loading placeholder immediately', async () => {
    // Use a promise that resolves after we check for "Loading..."
    let resolveApi: (val: SessionOutcomesSummary) => void;
    const pending = new Promise<SessionOutcomesSummary>((res) => { resolveApi = res; });
    mockClient.getSessionOutcomesSummary.mockReturnValue(pending);

    render(<RecentOutcomesPanel sessionId="ses-001" />);
    expect(screen.getByText('Loading…')).toBeInTheDocument();

    // Clean up: resolve so the effect settles before the test ends.
    resolveApi!(sampleSummary);
    await waitFor(() => expect(screen.queryByText('Loading…')).toBeNull());
  });

  it('renders kind chips and recent events after data loads', async () => {
    mockClient.getSessionOutcomesSummary.mockResolvedValue(sampleSummary);
    render(<RecentOutcomesPanel sessionId="ses-001" />);

    await waitFor(() => {
      // "tickets_resolved" appears in both the chip and the event list.
      expect(screen.getAllByText('tickets_resolved').length).toBeGreaterThanOrEqual(1);
    });

    // Kind chip counts.
    expect(screen.getByText('3')).toBeInTheDocument();
    // Recent event entries.
    expect(screen.getByText('Ticket #42 resolved')).toBeInTheDocument();
    expect(screen.getAllByText('hours_saved').length).toBeGreaterThanOrEqual(1);
  });

  it('renders "No outcomes yet." when by_kind is empty', async () => {
    const emptySummary: SessionOutcomesSummary = {
      ...sampleSummary,
      by_kind: {},
      recent: [],
    };
    mockClient.getSessionOutcomesSummary.mockResolvedValue(emptySummary);
    render(<RecentOutcomesPanel sessionId="ses-001" />);

    await waitFor(() => {
      expect(screen.getByText('No outcomes yet.')).toBeInTheDocument();
    });
  });

  it('shows an error message when the API call fails', async () => {
    mockClient.getSessionOutcomesSummary.mockRejectedValue(
      new Error('Network error'),
    );
    render(<RecentOutcomesPanel sessionId="ses-001" />);

    await waitFor(() => {
      expect(screen.getByRole('alert')).toBeInTheDocument();
      expect(screen.getByText('Network error')).toBeInTheDocument();
    });
  });

  it('renders the audit link with the correct session_id query param', () => {
    // Link is rendered unconditionally once sessionId is set.
    mockClient.getSessionOutcomesSummary.mockResolvedValue(sampleSummary);
    render(<RecentOutcomesPanel sessionId="ses-001" />);

    const link = screen.getByRole('link', { name: /open full outcomes audit/i });
    expect(link).toHaveAttribute(
      'href',
      '/admin/outcomes?session_id=ses-001',
    );
    expect(link).toHaveAttribute('target', '_blank');
  });

  it('prepends adminBaseUrl to the audit link', () => {
    mockClient.getSessionOutcomesSummary.mockResolvedValue(sampleSummary);
    render(
      <RecentOutcomesPanel
        sessionId="ses-001"
        adminBaseUrl="https://admin.example.com"
      />,
    );

    const link = screen.getByRole('link', { name: /open full outcomes audit/i });
    expect(link).toHaveAttribute(
      'href',
      'https://admin.example.com/admin/outcomes?session_id=ses-001',
    );
  });

  it('calls getSessionOutcomesSummary with the correct session id', async () => {
    mockClient.getSessionOutcomesSummary.mockResolvedValue(sampleSummary);
    render(<RecentOutcomesPanel sessionId="ses-999" />);

    await waitFor(() => {
      expect(mockClient.getSessionOutcomesSummary).toHaveBeenCalledWith('ses-999');
    });
  });
});
