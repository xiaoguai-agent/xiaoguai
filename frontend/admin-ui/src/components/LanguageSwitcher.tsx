import { useTranslation } from 'react-i18next';
import { SUPPORTED_LANGUAGES } from '../i18n/index';

export function LanguageSwitcher(): JSX.Element {
  const { i18n, t } = useTranslation();

  function handleChange(e: React.ChangeEvent<HTMLSelectElement>): void {
    void i18n.changeLanguage(e.target.value);
  }

  return (
    <label className="lang-switcher">
      <span className="lang-label">{t('language.label')}</span>
      <select value={i18n.language} onChange={handleChange} className="lang-select">
        {SUPPORTED_LANGUAGES.map((lang) => (
          <option key={lang.code} value={lang.code}>
            {lang.label}
          </option>
        ))}
      </select>
    </label>
  );
}
