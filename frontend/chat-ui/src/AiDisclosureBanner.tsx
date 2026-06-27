/**
 * AiDisclosureBanner — EU AI Act Art. 50(1) transparency banner.
 *
 * Behaviour:
 *  - Shown at the top of every new session (above HotlBanner when present).
 *  - Dismissible per-session via localStorage with a session-scoped key so
 *    it resurfaces on every new browser session / tab.
 *  - Operator-configurable via AiDisclosureConfig:
 *      enabled        — hide the banner entirely (default true)
 *      dismissible    — show the dismiss button (default true; set false for
 *                       regulated operators where the banner must always be visible)
 *      text_override  — replace the default translated text
 *      link_to_disclosure — render a "Learn more" link
 *
 * DEC-033 (single owner): there is no per-owner config endpoint, so
 * getAiDisclosureConfig returns the built-in global defaults (enabled=true,
 * dismissible=true). Pass the optional `config` prop to override.
 */

import { useEffect, useState } from 'react';
import type { AiDisclosureConfig } from '@xiaoguai/shared';
import { getAiDisclosureConfig, safeHref } from '@xiaoguai/shared';
import { getTranslations } from './i18n';

/** Per-session dismiss key stored in sessionStorage (resets on new tab/window). */
const DISMISS_KEY = 'xiaoguai.ai_disclosure.dismissed';

interface Props {
  /** Base URL for the API client (optional; defaults to same origin). */
  baseUrl?: string;
  /**
   * Optional pre-loaded config — skip the fetch. Useful for tests and
   * operator-embed scenarios where the config is already known.
   */
  config?: AiDisclosureConfig;
}

export function AiDisclosureBanner({ baseUrl, config: propConfig }: Props) {
  const [config, setConfig] = useState<AiDisclosureConfig | null>(propConfig ?? null);
  const [dismissed, setDismissed] = useState<boolean>(() => {
    try {
      return sessionStorage.getItem(DISMISS_KEY) === '1';
    } catch {
      return false;
    }
  });

  // Fetch the global config once on mount (skipped when propConfig is provided).
  useEffect(() => {
    if (propConfig !== undefined) return;
    void getAiDisclosureConfig({ baseUrl }).then(setConfig);
  }, [baseUrl, propConfig]);

  function handleDismiss() {
    try {
      sessionStorage.setItem(DISMISS_KEY, '1');
    } catch {
      // sessionStorage unavailable (private browsing edge case) — dismiss in-memory only.
    }
    setDismissed(true);
  }

  // Not yet loaded.
  if (!config) return null;
  // Operator has disabled the banner.
  if (!config.enabled) return null;
  // User already dismissed this session.
  if (dismissed) return null;

  const t = getTranslations();
  const bodyText = config.text_override ?? t.ai_disclosure.banner_text;
  // SEC-25: link_to_disclosure is operator/backend-provided config — only
  // whitelisted schemes become a link; otherwise the link is omitted (the
  // banner text itself still renders).
  const disclosureHref = safeHref(config.link_to_disclosure);

  return (
    <div
      className="ai-disclosure-banner"
      role="note"
      aria-label={t.ai_disclosure.banner_aria_label}
    >
      <span className="ai-disclosure-banner__text">
        {bodyText}
        {disclosureHref && (
          <>
            {' '}
            <a
              href={disclosureHref}
              target="_blank"
              rel="noopener noreferrer"
              className="ai-disclosure-banner__link"
            >
              {t.ai_disclosure.learn_more_label}
            </a>
          </>
        )}
      </span>
      {config.dismissible && (
        <button
          type="button"
          className="ai-disclosure-banner__dismiss"
          onClick={handleDismiss}
          aria-label={t.ai_disclosure.dismiss_label}
        >
          {t.ai_disclosure.dismiss_label}
        </button>
      )}
    </div>
  );
}
