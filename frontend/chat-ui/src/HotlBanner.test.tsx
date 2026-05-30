/**
 * HotlBanner — unit tests.
 *
 * Covers:
 *  - Renders with scope and queue link when `pending` is supplied.
 *  - Banner has `role="alert"` (non-dismissible, screen-reader accessible).
 *  - Operator queue link encodes the `request_id` correctly.
 *  - sprint-11 S11-3b inline decision flow (5 cases): Approve, Reject,
 *    Adjust validation, Submitting state, Error state.
 *  - sprint-12 S12-8 state machine (5 new cases): primary clear via
 *    `hotl_resolved` SSE, defensive 30s fallback, timeout-verdict
 *    annotation, sibling-tab conflict, fallback timer cancellation.
 */

import { render, screen, fireEvent, waitFor, act } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { HotlBanner } from './HotlBanner';
import type { HotlPendingState } from './HotlBanner';
import type { HotlResolvedEvent } from '@xiaoguai/shared';

const base: HotlPendingState = {
  request_id: 'req-abc-123',
  tool: 'fs_write',
  scope: 'tool_call.fs_write',
  args_redacted: { path: '/tmp/x' },
  expires_at: '2026-05-31T08:12:34Z',
};

describe('HotlBanner', () => {
  it('renders the title and scope', () => {
    render(<HotlBanner pending={base} />);

    expect(screen.getByRole('alert')).toBeInTheDocument();
    expect(screen.getByText('Human approval required')).toBeInTheDocument();
    // The scope is interpolated into the localized template.
    expect(
      screen.getByText(/The action tool_call.fs_write has been paused/),
    ).toBeInTheDocument();
  });

  it('renders a link to the operator approval queue with encoded request_id', () => {
    render(<HotlBanner pending={base} />);

    const link = screen.getByRole('link', { name: /open operator approval queue/i });
    expect(link).toBeInTheDocument();
    expect(link).toHaveAttribute(
      'href',
      '/hotl-queue?request_id=req-abc-123',
    );
    expect(link).toHaveAttribute('target', '_blank');
    expect(link).toHaveAttribute('rel', 'noopener noreferrer');
  });

  it('prepends adminBaseUrl to the queue link', () => {
    render(<HotlBanner pending={base} adminBaseUrl="https://admin.example.com" />);

    const link = screen.getByRole('link', { name: /open operator approval queue/i });
    expect(link).toHaveAttribute(
      'href',
      'https://admin.example.com/hotl-queue?request_id=req-abc-123',
    );
  });

  it('encodes special characters in request_id', () => {
    const specialId: HotlPendingState = { ...base, request_id: 'req a+b=c&d' };
    render(<HotlBanner pending={specialId} />);

    const link = screen.getByRole('link', { name: /open operator approval queue/i });
    expect(link.getAttribute('href')).toContain('req%20a%2Bb%3Dc%26d');
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

  // ── sprint-12 S12-8 — SSE-primary clear + 30s defensive fallback ────────

  describe('sprint-12 S12-8 — hotl_resolved primary clear', () => {
    // Fake-timer tests cannot use `waitFor` (its 50 ms polling needs real
    // time). Instead: drive React updates via `act()` + manual microtask
    // flushes, advance fake timers explicitly with vi.advanceTimersByTime.
    beforeEach(() => {
      vi.useFakeTimers({ shouldAdvanceTime: false });
    });
    afterEach(() => {
      vi.useRealTimers();
    });

    /** Build a resolved event matching the base pending request_id. */
    function resolvedEvent(
      overrides: Partial<HotlResolvedEvent> = {},
    ): HotlResolvedEvent {
      return {
        type: 'hotl_resolved',
        request_id: base.request_id,
        verdict: 'allow',
        decided_by: 'ops@acme.com',
        recorded_at: '2026-05-30T08:13:01Z',
        ...overrides,
      };
    }

    /**
     * Flush React's effect queue. The component's useEffect for the
     * resolved/cleared path is synchronous after a render; one act() with a
     * microtask drain is enough.
     */
    async function flushEffects(): Promise<void> {
      await act(async () => {
        await Promise.resolve();
      });
    }

    /** Submit the local decision and wait for `localSubmitted` to flip. */
    async function clickApproveAndAwaitSubmit(onDecision: ReturnType<typeof vi.fn>) {
      await act(async () => {
        fireEvent.click(screen.getByTestId('hotl-banner-approve'));
      });
      // The onDecision mock resolves on the next microtask; drain so the
      // post-await `setLocalSubmitted(true)` runs inside React's batch.
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(onDecision).toHaveBeenCalledTimes(1);
    }

    it('primary_clear_via_hotl_resolved_sse: matching SSE clears the banner immediately', async () => {
      const onCleared = vi.fn();
      const { rerender } = render(
        <HotlBanner pending={base} resolved={null} onCleared={onCleared} />,
      );
      expect(onCleared).not.toHaveBeenCalled();

      // Re-render with a matching resolved event — primary clear path.
      rerender(
        <HotlBanner pending={base} resolved={resolvedEvent()} onCleared={onCleared} />,
      );
      await flushEffects();

      expect(onCleared).toHaveBeenCalledTimes(1);
    });

    it('defensive_fallback_fires_at_30s_when_sse_silent: 30s timer clears after local submit when SSE never arrives', async () => {
      const onCleared = vi.fn();
      const onDecision = vi.fn().mockResolvedValue(undefined);
      render(
        <HotlBanner
          pending={base}
          resolved={null}
          onCleared={onCleared}
          onDecision={onDecision}
          decidedBy="alice@acme.com"
        />,
      );

      await clickApproveAndAwaitSubmit(onDecision);

      // Less than 30s: fallback NOT fired.
      await act(async () => {
        vi.advanceTimersByTime(29_000);
      });
      expect(onCleared).not.toHaveBeenCalled();

      // Cross the 30s threshold: fallback fires.
      await act(async () => {
        vi.advanceTimersByTime(2_000);
      });
      expect(onCleared).toHaveBeenCalledTimes(1);
    });

    it('timeout_verdict_shows_annotation_for_3s before clearing', async () => {
      const onCleared = vi.fn();
      const { rerender } = render(
        <HotlBanner pending={base} resolved={null} onCleared={onCleared} />,
      );

      // Re-render with timeout verdict — annotation appears, banner not yet cleared.
      rerender(
        <HotlBanner
          pending={base}
          resolved={resolvedEvent({ verdict: 'timeout', decided_by: null })}
          onCleared={onCleared}
        />,
      );
      await flushEffects();

      expect(
        screen.getByTestId('hotl-banner-timeout-annotation'),
      ).toBeInTheDocument();
      expect(
        screen.getByText(/Decision timed out — tool call denied/),
      ).toBeInTheDocument();
      // Annotation visible, not yet cleared.
      expect(onCleared).not.toHaveBeenCalled();

      // Advance 3s — annotation drives clear.
      await act(async () => {
        vi.advanceTimersByTime(3_100);
      });
      expect(onCleared).toHaveBeenCalledTimes(1);
    });

    it('sibling_tab_conflict_reverts_local_state when decided_by differs from local actor', async () => {
      const onCleared = vi.fn();
      const onDecision = vi.fn().mockResolvedValue(undefined);
      const { rerender } = render(
        <HotlBanner
          pending={base}
          resolved={null}
          onCleared={onCleared}
          onDecision={onDecision}
          decidedBy="alice@acme.com"
        />,
      );

      // Alice clicks Approve — local submit succeeds, banner enters submitting state.
      await clickApproveAndAwaitSubmit(onDecision);

      // SSE then arrives with a DIFFERENT decided_by — sibling tab raced ahead.
      rerender(
        <HotlBanner
          pending={base}
          resolved={resolvedEvent({ decided_by: 'bob@acme.com' })}
          onCleared={onCleared}
          onDecision={onDecision}
          decidedBy="alice@acme.com"
        />,
      );
      await flushEffects();

      // Conflict toast surfaces.
      expect(
        screen.getByTestId('hotl-banner-conflict-toast'),
      ).toBeInTheDocument();
      // Local submitting state reverted (no longer disabled).
      expect(screen.getByTestId('hotl-banner-approve')).not.toBeDisabled();
      // Banner clears via SSE (allow verdict from sibling).
      expect(onCleared).toHaveBeenCalledTimes(1);
    });

    it('sse_resolved_cancels_pending_fallback_timer: only one onCleared call', async () => {
      const onCleared = vi.fn();
      const onDecision = vi.fn().mockResolvedValue(undefined);
      const { rerender } = render(
        <HotlBanner
          pending={base}
          resolved={null}
          onCleared={onCleared}
          onDecision={onDecision}
          decidedBy="alice@acme.com"
        />,
      );

      // Click Approve — fallback timer is now ticking.
      await clickApproveAndAwaitSubmit(onDecision);

      // Advance to just before the 30s threshold.
      await act(async () => {
        vi.advanceTimersByTime(20_000);
      });
      expect(onCleared).not.toHaveBeenCalled();

      // SSE arrives — primary clear, fallback must be cancelled.
      rerender(
        <HotlBanner
          pending={base}
          resolved={resolvedEvent({ decided_by: 'alice@acme.com' })}
          onCleared={onCleared}
          onDecision={onDecision}
          decidedBy="alice@acme.com"
        />,
      );
      await flushEffects();
      expect(onCleared).toHaveBeenCalledTimes(1);

      // Advance past 30s total — fallback must NOT fire a second time
      // (the SSE primary clear cancelled it).
      await act(async () => {
        vi.advanceTimersByTime(20_000);
      });
      expect(onCleared).toHaveBeenCalledTimes(1);
    });
  });
});
