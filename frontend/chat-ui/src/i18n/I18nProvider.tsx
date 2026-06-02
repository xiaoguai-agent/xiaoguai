/**
 * Reactive i18n layer over the static `getTranslations` helper.
 *
 * The base helper auto-detects the browser locale; this adds a runtime
 * language switcher. `I18nProvider` holds the active locale in state
 * (seeded from the stored choice), persists changes to localStorage, and
 * re-renders the whole tree when it changes so every `useI18n().t` reflects
 * the new language immediately.
 */

import { createContext, useCallback, useContext, useMemo, useState } from 'react';
import type { ReactNode } from 'react';
import { getStoredLocale, getTranslations, setStoredLocale } from './index';
import type { Locale } from './index';

type Translations = ReturnType<typeof getTranslations>;

interface I18nContextValue {
  locale: Locale;
  setLocale: (locale: Locale) => void;
  /** Resolved translation bundle for the active locale. */
  t: Translations;
}

const I18nContext = createContext<I18nContextValue | null>(null);

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocaleState] = useState<Locale>(() => getStoredLocale());

  const setLocale = useCallback((next: Locale) => {
    setStoredLocale(next);
    setLocaleState(next);
  }, []);

  const value = useMemo<I18nContextValue>(
    () => ({ locale, setLocale, t: getTranslations(locale) }),
    [locale, setLocale],
  );

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

/**
 * Access the active locale, a setter, and the resolved translation bundle.
 * Throws if used outside `I18nProvider` so the wiring mistake is loud.
 */
export function useI18n(): I18nContextValue {
  const ctx = useContext(I18nContext);
  if (ctx === null) {
    throw new Error('useI18n must be used within <I18nProvider>');
  }
  return ctx;
}
