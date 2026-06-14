import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';
import LanguageDetector from 'i18next-browser-languagedetector';

import enTranslation from './locales/en/translation.json';
import zhCNTranslation from './locales/zh-CN/translation.json';
import jaTranslation from './locales/ja/translation.json';

export const SUPPORTED_LANGUAGES = [
  { code: 'en', label: 'English' },
  { code: 'zh-CN', label: '中文' },
  { code: 'ja', label: '日本語' },
] as const;

export type SupportedLanguage = (typeof SUPPORTED_LANGUAGES)[number]['code'];

const resources = {
  en: { translation: enTranslation },
  'zh-CN': { translation: zhCNTranslation },
  ja: { translation: jaTranslation },
};

void i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    resources,
    fallbackLng: 'en',
    supportedLngs: ['en', 'zh-CN', 'ja'],
    detection: {
      order: ['localStorage', 'navigator'],
      caches: ['localStorage'],
      // Shared key with chat-ui (its i18n LOCALE_STORAGE_KEY) so the operator's
      // language choice is GLOBAL across both SPAs — they live on the same
      // :7600 origin, so one localStorage key keeps chat + admin in sync.
      lookupLocalStorage: 'xiaoguai.locale',
    },
    interpolation: {
      escapeValue: false,
    },
  });

export default i18n;
