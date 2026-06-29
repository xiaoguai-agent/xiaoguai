/**
 * Sprint-10b S10b-7 — i18n key parity test (REQ-UI-005 enforcement).
 *
 * Every translation key present in any locale must be present in every
 * other locale. A missing key is a build-time failure — the rendered
 * value would otherwise fall back to the i18next default (the raw key
 * string), which leaks English internals to non-English operators.
 *
 * This is the structural half of REQ-UI-005. The companion piece —
 * Axe-core accessibility violations failing CI — lives in `frontend/e2e`
 * via S10b-8.
 */
import { describe, expect, it } from 'vitest';
import enTranslation from './locales/en/translation.json';
import zhCNTranslation from './locales/zh-CN/translation.json';

type TranslationBundle = Record<string, unknown>;

/**
 * Flatten a nested translation bundle to dotted-path keys.
 *
 * Example:
 *   { auth: { sessionExpired: "…" } } → ["auth.sessionExpired"]
 */
function flattenKeys(bundle: TranslationBundle, prefix = ''): Set<string> {
  const keys = new Set<string>();
  for (const [key, value] of Object.entries(bundle)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
      const nested = flattenKeys(value as TranslationBundle, path);
      for (const k of nested) keys.add(k);
    } else {
      keys.add(path);
    }
  }
  return keys;
}

interface LocaleBundle {
  code: string;
  bundle: TranslationBundle;
}

const LOCALES: LocaleBundle[] = [
  { code: 'en', bundle: enTranslation as TranslationBundle },
  { code: 'zh-CN', bundle: zhCNTranslation as TranslationBundle },
];

describe('i18n key parity', () => {
  it('all locales expose the same set of keys', () => {
    const keysByLocale = LOCALES.map(({ code, bundle }) => ({
      code,
      keys: flattenKeys(bundle),
    }));

    // Union of all keys across all locales = the canonical set.
    const allKeys = new Set<string>();
    for (const { keys } of keysByLocale) {
      for (const k of keys) allKeys.add(k);
    }

    // For each locale, report what's missing.
    const missing: { locale: string; absent: string[] }[] = [];
    for (const { code, keys } of keysByLocale) {
      const absent: string[] = [];
      for (const k of allKeys) {
        if (!keys.has(k)) absent.push(k);
      }
      if (absent.length > 0) {
        missing.push({ locale: code, absent: absent.sort() });
      }
    }

    expect(missing).toEqual([]);
  });

  it('every locale has at least one key — bundles loaded successfully', () => {
    for (const { code, bundle } of LOCALES) {
      const keys = flattenKeys(bundle);
      expect(keys.size, `locale ${code} should contain translation keys`).toBeGreaterThan(0);
    }
  });

  it('no locale has an empty string value (a structural gap)', () => {
    const emptyKeys: { locale: string; key: string }[] = [];

    function walk(bundle: TranslationBundle, prefix: string, code: string): void {
      for (const [key, value] of Object.entries(bundle)) {
        const path = prefix ? `${prefix}.${key}` : key;
        if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
          walk(value as TranslationBundle, path, code);
        } else if (typeof value === 'string' && value.length === 0) {
          emptyKeys.push({ locale: code, key: path });
        }
      }
    }

    for (const { code, bundle } of LOCALES) {
      walk(bundle, '', code);
    }

    expect(emptyKeys).toEqual([]);
  });

  it('all values are strings (no accidental numbers / booleans)', () => {
    const nonStringValues: { locale: string; key: string; type: string }[] = [];

    function walk(bundle: TranslationBundle, prefix: string, code: string): void {
      for (const [key, value] of Object.entries(bundle)) {
        const path = prefix ? `${prefix}.${key}` : key;
        if (value === null) {
          nonStringValues.push({ locale: code, key: path, type: 'null' });
        } else if (typeof value === 'object' && !Array.isArray(value)) {
          walk(value as TranslationBundle, path, code);
        } else if (typeof value !== 'string') {
          nonStringValues.push({ locale: code, key: path, type: typeof value });
        }
      }
    }

    for (const { code, bundle } of LOCALES) {
      walk(bundle, '', code);
    }

    expect(nonStringValues).toEqual([]);
  });
});
