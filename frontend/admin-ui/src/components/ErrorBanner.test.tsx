/**
 * DEC-041 (frontend half) — tests for the shared <ErrorBanner>. Uses the real
 * admin-ui i18n instance so the rendered copy matches the production bundle.
 */
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { I18nextProvider } from 'react-i18next';
import i18n from '../i18n/index';
import { ErrorBanner, type ErrorBannerProps } from './ErrorBanner';

function renderBanner(props: ErrorBannerProps) {
  return render(
    <I18nextProvider i18n={i18n}>
      <ErrorBanner {...props} />
    </I18nextProvider>,
  );
}

describe('<ErrorBanner>', () => {
  it('renders nothing when message is falsy', () => {
    const { container } = renderBanner({ message: null });
    expect(container).toBeEmptyDOMElement();
  });

  it('renders an accessible alert containing the interpolated message', () => {
    renderBanner({ message: 'boom' });
    const alert = screen.getByRole('alert');
    expect(alert).toHaveTextContent('boom');
  });

  it('omits the retry button when no handler is given', () => {
    renderBanner({ message: 'boom' });
    expect(screen.queryByRole('button')).toBeNull();
  });

  it('shows a retry button that fires onRetry', () => {
    const onRetry = vi.fn();
    renderBanner({ message: 'boom', onRetry });
    fireEvent.click(screen.getByRole('button'));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });
});
