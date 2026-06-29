/**
 * Tests for the runtime language-switcher persistence layer added alongside
 * the in-app 中/EN selector. The base helper only auto-detected the
 * browser locale; an explicit stored choice must now win and round-trip.
 */
import { afterEach, describe, expect, it } from 'vitest';
import { getStoredLocale, getTranslations, setStoredLocale } from './index';

afterEach(() => {
  localStorage.clear();
});

describe('locale persistence', () => {
  it('round-trips an explicit choice through localStorage', () => {
    setStoredLocale('zh-CN');
    expect(getStoredLocale()).toBe('zh-CN');
    setStoredLocale('en');
    expect(getStoredLocale()).toBe('en');
  });

  it('getTranslations() with no arg follows the stored choice', () => {
    setStoredLocale('zh-CN');
    // A translated main-UI string resolves to the stored locale's bundle.
    expect(getTranslations().ui.send).toBe('发送');
    setStoredLocale('en');
    expect(getTranslations().ui.send).toBe('Send');
  });

  it('ignores a non-bundled stored value and falls back', () => {
    localStorage.setItem('xiaoguai.locale', 'fr');
    // Not a bundled locale → falls back to browser detection (en in jsdom).
    expect(getStoredLocale()).toBe('en');
  });
});
