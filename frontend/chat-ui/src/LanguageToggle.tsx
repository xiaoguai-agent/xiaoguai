/**
 * LanguageToggle — sidebar language switcher (中文 / English).
 *
 * Sits in the sidebar footer next to the theme toggle. Changing it persists
 * the choice (localStorage) and re-renders the app in the new language via
 * the i18n context.
 */

import { AVAILABLE_LOCALES } from './i18n';
import type { Locale } from './i18n';
import { useI18n } from './i18n/I18nProvider';

export function LanguageToggle() {
  const { locale, setLocale, t } = useI18n();

  return (
    <label className="lang-toggle">
      <span className="lang-toggle-label">{t.ui.language_label}</span>
      <select
        className="lang-toggle-select"
        value={locale}
        aria-label={t.ui.language_label}
        onChange={(e) => setLocale(e.target.value as Locale)}
      >
        {AVAILABLE_LOCALES.map(({ code, label }) => (
          <option key={code} value={code}>
            {label}
          </option>
        ))}
      </select>
    </label>
  );
}
