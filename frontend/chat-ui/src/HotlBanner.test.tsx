/**
 * HotlBanner — unit tests.
 *
 * Covers:
 *  - Renders with scope, reason, and escalation link when `pending` is supplied.
 *  - Banner has `role="alert"` (non-dismissible, screen-reader accessible).
 *  - Operator queue link encodes the escalation_id correctly.
 *  - Reason paragraph is omitted when `reason` is an empty string.
 *  - sprint-11 S11-3b inline decision flow (5 cases): Approve, Reject,
 *    Adjust validation, Submitting state, Error state.
 */

import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { describe, it, expect, vi } from 'vitest';
import { HotlBanner } from './HotlBanner';
import type { HotlPendingState } from './HotlBanner';

const base: HotlPendingState = {
  escalation_id: 'esc-abc-123',
  scope: 'fs_write',
  reason: 'Policy rule: no filesystem writes outside /tmp.',
};

describe('HotlBanner', () => {
  it('renders the title and scope', () => {
    render(<HotlBanner pending={base} />);

    expect(screen.getByRole('alert')).toBeInTheDocument();
    expect(screen.getByText('Human approval required')).toBeInTheDocument();
    // The scope is interpolated into the localized template.
    expect(
      screen.getByText(/The action fs_write has been paused/),
    ).toBeInTheDocument();
  });

  it('renders the reason text', () => {
    render(<HotlBanner pending={base} />);

    expect(
      screen.getByText('Policy rule: no filesystem writes outside /tmp.'),
    ).toBeInTheDocument();
  });

  it('omits the reason paragraph when reason is empty', () => {
    const noBanner = { ...base, reason: '' };
    render(<HotlBanner pending={noBanner} />);

    expect(
      screen.queryByText('Policy rule: no filesystem writes outside /tmp.'),
    ).toBeNull();
  });

  it('renders a link to the operator approval queue with encoded escalation_id', () => {
    render(<HotlBanner pending={base} />);

    const link = screen.getByRole('link', { name: /open operator approval queue/i });
    expect(link).toBeInTheDocument();
    expect(link).toHaveAttribute(
      'href',
      '/hotl-queue?escalation_id=esc-abc-123',
    );
    expect(link).toHaveAttribute('target', '_blank');
    expect(link).toHaveAttribute('rel', 'noopener noreferrer');
  });

  it('prepends adminBaseUrl to the queue link', () => {
    render(<HotlBanner pending={base} adminBaseUrl="https://admin.example.com" />);

    const link = screen.getByRole('link', { name: /open operator approval queue/i });
    expect(link).toHaveAttribute(
      'href',
      'https://admin.example.com/hotl-queue?escalation_id=esc-abc-123',
    );
  });

  it('encodes special characters in escalation_id', () => {
    const specialId: HotlPendingState = { ...base, escalation_id: 'esc a+b=c&d' };
    render(<HotlBanner pending={specialId} />);

    const link = screen.getByRole('link', { name: /open operator approval queue/i });
    expect(link.getAttribute('href')).toContain('esc%20a%2Bb%3Dc%26d');
  });

  it('does not render inline buttons when onDecision is not provided', () => {
    render(<HotlBanner pending={base} />);
    expect(screen.queryByTestId('hotl-banner-approve')).toBeNull();
    expect(screen.queryByTestId('hotl-banner-reject')).toBeNull();
    expect(screen.queryByTestId('hotl-banner-adjust')).toBeNull();
  });

  // ── sprint-11 S11-3b — inline decision flow ─────────────────────────────

  it('Approve click invokes onDecision("allow", undefined) when adjust panel is closed', async () => {
    const onDecision = vi.fn().mockResolvedValue(undefined);
    render(<HotlBanner pending={base} onDecision={onDecision} />);

    fireEvent.click(screen.getByTestId('hotl-banner-approve'));
    await waitFor(() => expect(onDecision).toHaveBeenCalledTimes(1));
    expect(onDecision).toHaveBeenCalledWith('allow', undefined);
  });

  it('Reject click invokes onDecision("deny", undefined) when adjust panel is closed', async () => {
    const onDecision = vi.fn().mockResolvedValue(undefined);
    render(<HotlBanner pending={base} onDecision={onDecision} />);

    fireEvent.click(screen.getByTestId('hotl-banner-reject'));
    await waitFor(() => expect(onDecision).toHaveBeenCalledTimes(1));
    expect(onDecision).toHaveBeenCalledWith('deny', undefined);
  });

  it('Adjust panel: clicking Approve without rationale shows validation error and does not call onDecision', async () => {
    const onDecision = vi.fn().mockResolvedValue(undefined);
    render(<HotlBanner pending={base} onDecision={onDecision} />);

    fireEvent.click(screen.getByTestId('hotl-banner-adjust'));
    // Sub-panel is now open. Fill max_count to satisfy "at least one budget"
    // but leave rationale empty to trigger the validation error.
    const maxCountInput = screen.getByLabelText(/Max calls/);
    fireEvent.change(maxCountInput, { target: { value: '5' } });

    fireEvent.click(screen.getByTestId('hotl-banner-approve'));

    await waitFor(() => {
      expect(screen.getByText(/rationale is required/i)).toBeInTheDocument();
    });
    expect(onDecision).not.toHaveBeenCalled();
  });

  it('disables all action buttons and shows "Submitting…" while onDecision is in flight', async () => {
    let resolveFn: (() => void) | undefined;
    const onDecision = vi.fn(
      () =>
        new Promise<void>((res) => {
          resolveFn = res;
        }),
    );
    render(<HotlBanner pending={base} onDecision={onDecision} />);

    fireEvent.click(screen.getByTestId('hotl-banner-approve'));

    await waitFor(() => {
      const approve = screen.getByTestId('hotl-banner-approve');
      expect(approve).toBeDisabled();
      expect(approve.textContent).toMatch(/Submitting/);
    });
    expect(screen.getByTestId('hotl-banner-reject')).toBeDisabled();
    expect(screen.getByTestId('hotl-banner-adjust')).toBeDisabled();

    resolveFn?.();
    await waitFor(() => expect(onDecision).toHaveBeenCalledTimes(1));
  });

  it('surfaces submission failure: error message rendered, buttons re-enabled', async () => {
    const onDecision = vi.fn().mockRejectedValue(new Error('HTTP 409: duplicate'));
    render(<HotlBanner pending={base} onDecision={onDecision} />);

    fireEvent.click(screen.getByTestId('hotl-banner-approve'));

    await waitFor(() => {
      expect(
        screen.getByText(/Decision submission failed: HTTP 409: duplicate/),
      ).toBeInTheDocument();
    });
    expect(screen.getByTestId('hotl-banner-approve')).not.toBeDisabled();
    expect(screen.getByTestId('hotl-banner-reject')).not.toBeDisabled();
  });
});
