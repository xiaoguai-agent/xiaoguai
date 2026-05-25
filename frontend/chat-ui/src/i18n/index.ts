/**
 * Minimal i18n helper — resolves the browser locale to one of the bundled
 * translation files without pulling in a full i18next dependency.
 *
 * Supported locales: en (default), zh-CN, ja.
 * Falls back to en for any locale not in the map.
 */

import en from './locales/en/translation.json';
import zhCN from './locales/zh-CN/translation.json';
import ja from './locales/ja/translation.json';

type Locale = 'en' | 'zh-CN' | 'ja';

interface TranslationShape {
  ai_disclosure: {
    banner_text: string;
    dismiss_label: string;
    learn_more_label: string;
    banner_aria_label: string;
  };
}

const bundles: Record<Locale, TranslationShape> = { en, 'zh-CN': zhCN, ja };

function detectLocale(): Locale {
  const nav = typeof navigator !== 'undefined' ? navigator.language : 'en';
  if (nav === 'zh-CN' || nav.startsWith('zh')) return 'zh-CN';
  if (nav === 'ja' || nav.startsWith('ja')) return 'ja';
  return 'en';
}

export function getTranslations(locale?: Locale): TranslationShape {
  const resolved: Locale = locale ?? detectLocale();
  return bundles[resolved] ?? bundles['en'];
}
