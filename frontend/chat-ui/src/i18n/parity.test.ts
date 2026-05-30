/**
 * Sprint-11 S11-2c — chat-ui i18n key parity test (REQ-UI-005 enforcement).
 *
 * Sibling to `frontend/admin-ui/src/i18n/parity.test.ts` (sprint-10b S10b-7).
 * Sprint-11 added `chat.sse.*` (S11-2b) and `chat.hotl.*` (S11-3b) key
 * families across three locales — without a parity guard, a missing zh-CN /
 * ja translation ships silently and i18next falls back to the raw key string,
 * leaking English internals to non-English operators.
 *
 * The structural contract is identical to admin-ui's: every key present in
 * any locale must be present in every other locale; no empty strings; all
 * values are strings.
 */
import { describe, expect, it } from 'vitest';
import enTranslation from './locales/en/translation.json';
import zhCNTranslation from './locales/zh-CN/translation.json';
import jaTranslation from './locales/ja/translation.json';

type TranslationBundle = Record<string, unknown>;

/**
 * Flatten a nested translation bundle to dotted-path keys.
 *
 * Example:
 *   { chat: { sse: { reconnecting: "…" } } } → ["chat.sse.reconnecting"]
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
  { code: 'ja', bundle: jaTranslation as TranslationBundle },
];

describe('chat-ui i18n key parity', () => {
  it('all locales expose the same set of keys', () => {
    const keysByLocale = LOCALES.map(({ code, bundle }) => ({
      code,
      keys: flattenKeys(bundle),
    }));

    const allKeys = new Set<string>();
    for (const { keys } of keysByLocale) {
      for (const k of keys) allKeys.add(k);
    }

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
