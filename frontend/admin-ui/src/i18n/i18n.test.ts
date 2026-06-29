/**
 * i18n tests — C19.
 *
 * 1. Each locale (en, zh-CN) has a non-empty translation for a sample key.
 * 2. Switching locale changes the resolved value.
 * 3. A missing key falls back to the English (fallbackLng) value.
 */

import { describe, it, expect, beforeEach } from 'vitest';
import i18n from './index';
import enTranslation from './locales/en/translation.json';
import zhCNTranslation from './locales/zh-CN/translation.json';

// Ensure i18n is initialised before tests run.
beforeEach(async () => {
  // Force language back to English between tests.
  await i18n.changeLanguage('en');
});

describe('i18n locale loading', () => {
  it('en locale resolves common.refresh', () => {
    expect(i18n.t('common.refresh')).toBe(enTranslation.common.refresh);
    expect(i18n.t('common.refresh')).toBeTruthy();
  });

  it('zh-CN locale resolves common.refresh', async () => {
    await i18n.changeLanguage('zh-CN');
    expect(i18n.t('common.refresh')).toBe(zhCNTranslation.common.refresh);
    expect(i18n.t('common.refresh')).toBeTruthy();
  });
});

describe('locale switching', () => {
  it('switching from en to zh-CN changes nav.today translation', async () => {
    const enValue = i18n.t('nav.today');
    await i18n.changeLanguage('zh-CN');
    const zhValue = i18n.t('nav.today');
    expect(enValue).toBe(enTranslation.nav.today);
    expect(zhValue).toBe(zhCNTranslation.nav.today);
    expect(enValue).not.toBe(zhValue);
  });

  it('switching back to en restores English strings', async () => {
    await i18n.changeLanguage('zh-CN');
    await i18n.changeLanguage('en');
    expect(i18n.t('nav.today')).toBe(enTranslation.nav.today);
  });
});

describe('missing key fallback', () => {
  it('falls back to en when key is missing in zh-CN', async () => {
    // Add a resource key only to English to simulate a missing translation.
    i18n.addResourceBundle('en', 'translation', {
      __test__: { only_in_en: 'English only value' },
    }, true, true);

    await i18n.changeLanguage('zh-CN');
    // i18next falls back to en when the key is absent in zh-CN.
    const result = i18n.t('__test__.only_in_en');
    expect(result).toBe('English only value');
  });

  it('returns the key itself when missing from all locales', async () => {
    await i18n.changeLanguage('en');
    const result = i18n.t('__nonexistent_key_xyz__');
    // i18next returns the key string when not found anywhere.
    expect(result).toBe('__nonexistent_key_xyz__');
  });
});

describe('interpolation', () => {
  it('interpolates message in common.failed', () => {
    const result = i18n.t('common.failed', { message: 'network error' });
    expect(result).toContain('network error');
  });

  it('resolves pane.audit.empty without interpolation', () => {
    const result = i18n.t('pane.audit.empty');
    expect(result).toBeTruthy();
    expect(result).not.toBe('pane.audit.empty');
  });
});

describe('locale completeness', () => {
  const sampleKeys = [
    'common.refresh',
    'common.loading',
    'nav.today',
    'nav.scheduler',
    'nav.eval',
    'nav.usage',
    'pane.today.title',
    'pane.usage.title',
    'pane.scheduler.title',
    'pane.eval.title',
    'pane.audit.title',
    'pane.mcp_servers.title',
    'pane.marketplace.title',
    'pane.providers.title',
  ] as const;

  for (const lang of ['en', 'zh-CN'] as const) {
    it(`all sample keys resolve non-empty in ${lang}`, async () => {
      await i18n.changeLanguage(lang);
      for (const key of sampleKeys) {
        const value = i18n.t(key);
        expect(value, `key "${key}" missing in ${lang}`).toBeTruthy();
        // A resolved key should never equal the key path itself (that means it's missing).
        expect(value, `key "${key}" not translated in ${lang}`).not.toBe(key);
      }
    });
  }
});
