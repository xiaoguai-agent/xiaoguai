import { useTranslation } from 'react-i18next';

/**
 * Consistent, accessible error banner for admin panes (DEC-041, frontend half).
 *
 * Replaces the ~3 ad-hoc patterns panes used for failures — raw
 * `<div className="error">{message}</div>`, an i18n-wrapped variant, or no
 * display at all (silent failure) — with one `role="alert"` + i18n'd banner.
 * Renders nothing when `message` is falsy, so callers can drop it in
 * unconditionally:
 *
 *   `<ErrorBanner message={error} onRetry={reload} />`
 */
export interface ErrorBannerProps {
  /** Error text; when null / undefined / empty the banner renders nothing. */
  message: string | null | undefined;
  /** Optional retry handler — renders a "Retry" button when provided. */
  onRetry?: () => void;
}

export function ErrorBanner({ message, onRetry }: ErrorBannerProps): JSX.Element | null {
  const { t } = useTranslation();
  if (!message) return null;
  return (
    <div className="error" role="alert">
      <span>{t('common.failed', { message })}</span>
      {onRetry && (
        <button type="button" className="error-retry" onClick={onRetry}>
          {t('common.retry')}
        </button>
      )}
    </div>
  );
}
