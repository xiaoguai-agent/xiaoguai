/**
 * SseReconnectBanner — unit tests (sprint-11 S11-2b).
 *
 * Coverage:
 *  1. Renders with stable data-testid + ARIA live status.
 *  2. Interpolates attempt + secs into the localized template.
 *  3. Cancel button is rendered only when onCancel is provided and fires
 *     the supplied handler.
 *  4. Rounds nextDelayMs to the nearest second, never displaying "0s".
 */

import type { ReactElement } from 'react';
import { describe, it, expect, vi } from 'vitest';
import { render as rtlRender, screen, fireEvent } from '@testing-library/react';

import { SseReconnectBanner } from './SseReconnectBanner';
import { I18nProvider } from './i18n/I18nProvider';

/** The banner reads translations via `useI18n()`, so it must render inside the provider. */
function render(ui: ReactElement) {
  return rtlRender(<I18nProvider>{ui}</I18nProvider>);
}

describe('SseReconnectBanner', () => {
  it('renders with the e2e contract data-testid + polite ARIA live region', () => {
    render(<SseReconnectBanner attempt={1} nextDelayMs={1000} />);
    const banner = screen.getByTestId('sse-reconnect-banner');
    expect(banner).toBeInTheDocument();
    expect(banner).toHaveAttribute('role', 'status');
    expect(banner).toHaveAttribute('aria-live', 'polite');
  });

  it('interpolates the attempt count and seconds-until-next-retry into the label', () => {
    render(<SseReconnectBanner attempt={2} nextDelayMs={4000} />);
    expect(screen.getByTestId('sse-reconnect-banner').textContent).toMatch(/2/);
    expect(screen.getByTestId('sse-reconnect-banner').textContent).toMatch(/4/);
  });

  it('omits the cancel button when onCancel is not provided', () => {
    render(<SseReconnectBanner attempt={1} nextDelayMs={1000} />);
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  });

  it('fires onCancel when the user clicks the cancel button', () => {
    const onCancel = vi.fn();
    render(
      <SseReconnectBanner attempt={1} nextDelayMs={1000} onCancel={onCancel} />,
    );
    fireEvent.click(screen.getByRole('button'));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it('rounds sub-second delays up to 1s so the user never sees "0s"', () => {
    render(<SseReconnectBanner attempt={1} nextDelayMs={400} />);
    expect(screen.getByTestId('sse-reconnect-banner').textContent).toMatch(/1/);
    expect(screen.getByTestId('sse-reconnect-banner').textContent).not.toMatch(/0s/);
  });
});
