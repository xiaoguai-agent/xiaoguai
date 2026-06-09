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

export type Locale = 'en' | 'zh-CN' | 'ja';

/** Locales offered in the language switcher, with native display labels. */
export const AVAILABLE_LOCALES: ReadonlyArray<{ code: Locale; label: string }> = [
  { code: 'en', label: 'English' },
  { code: 'zh-CN', label: '中文' },
  { code: 'ja', label: '日本語' },
];

/** localStorage key holding the operator's explicit language choice. */
const LOCALE_STORAGE_KEY = 'xiaoguai.locale';

interface TranslationShape {
  ai_disclosure: {
    banner_text: string;
    dismiss_label: string;
    learn_more_label: string;
    banner_aria_label: string;
  };
  /** Main chat-ui surface strings (sidebar, welcome, composer). */
  ui: {
    new_chat: string;
    skills: string;
    admin: string;
    language_label: string;
    no_sessions: string;
    welcome_title: string;
    welcome_subtitle: string;
    composer_placeholder: string;
    composer_hint: string;
    stop: string;
    send: string;
    stop_generating: string;
    send_message: string;
    thinking: string;
    branch: string;
    branch_title: string;
    branch_label: string;
    /** Opening prompts on the welcome screen (Gemini-style chips). */
    suggestions: {
      summarize_doc: string;
      write_shell: string;
      analyze_cve: string;
      explain_codebase: string;
    };
    /** T3.5 — expert picker (header chip + popover). */
    expert: {
      pick_label: string;
      chip_title: string;
      panel_title: string;
      remove: string;
      filter_placeholder: string;
      group_personas: string;
      group_teams: string;
      empty_group: string;
      kind_persona: string;
      kind_team: string;
      suggest_label: string;
      suggest_placeholder: string;
      suggest_button: string;
      suggest_searching: string;
      suggest_empty: string;
      error_load: string;
      error_attach: string;
      error_detach: string;
      error_suggest: string;
    };
  };
  chat: {
    sse: {
      reconnecting: string;
      cancel_reconnect: string;
      gave_up: string;
    };
    hotl: {
      title: string;
      scope_label: string;
      btn_approve: string;
      btn_reject: string;
      btn_adjust: string;
      submitting: string;
      submit_failed: string;
      policy_tighten: string;
      policy_loosen: string;
      window_seconds_label: string;
      max_count_label: string;
      max_usd_label: string;
      rationale_label: string;
      review_link: string;
      timeout_annotation: string;
      conflict_toast: string;
    };
    /** /loop slash-command help / confirmation / status / error strings. */
    loop: {
      help_title: string;
      help_create: string;
      help_status: string;
      help_cancel: string;
      help_help: string;
      need_session: string;
      confirm_title: string;
      confirm_prompt: string;
      confirm_pacing: string;
      btn_arm: string;
      btn_cancel: string;
      not_armed: string;
      armed: string;
      status_header: string;
      status_empty: string;
      status_line: string;
      cancel_none: string;
      cancelled: string;
      error: string;
    };
  };
}

const bundles: Record<Locale, TranslationShape> = {
  en: en as unknown as TranslationShape,
  'zh-CN': zhCN as unknown as TranslationShape,
  ja: ja as unknown as TranslationShape,
};

function detectLocale(): Locale {
  const nav = typeof navigator !== 'undefined' ? navigator.language : 'en';
  if (nav === 'zh-CN' || nav.startsWith('zh')) return 'zh-CN';
  if (nav === 'ja' || nav.startsWith('ja')) return 'ja';
  return 'en';
}

/** True when `value` is one of the bundled locales. */
function isLocale(value: string | null): value is Locale {
  return value === 'en' || value === 'zh-CN' || value === 'ja';
}

/**
 * Resolve the active locale: an explicit stored choice wins over the
 * auto-detected browser locale. Safe when `localStorage` is unavailable.
 */
export function getStoredLocale(): Locale {
  try {
    const stored = typeof localStorage !== 'undefined' ? localStorage.getItem(LOCALE_STORAGE_KEY) : null;
    if (isLocale(stored)) return stored;
  } catch {
    // localStorage can throw (private mode / disabled) — fall back to detection.
  }
  return detectLocale();
}

/** Persist the operator's explicit language choice. */
export function setStoredLocale(locale: Locale): void {
  try {
    if (typeof localStorage !== 'undefined') localStorage.setItem(LOCALE_STORAGE_KEY, locale);
  } catch {
    // Best-effort: a non-persisted switch still applies for the session.
  }
}

export function getTranslations(locale?: Locale): TranslationShape {
  const resolved: Locale = locale ?? getStoredLocale();
  return bundles[resolved] ?? bundles['en'];
}

/**
 * Minimal `{{var}}` interpolation. Substitutes every `{{key}}` in `template`
 * with the matching string/number from `vars`. Missing keys are left as-is
 * so a parity-test failure is easy to spot at runtime.
 */
export function interpolate(
  template: string,
  vars: Record<string, string | number>,
): string {
  return template.replace(/\{\{(\w+)\}\}/g, (_, key: string) => {
    const v = vars[key];
    return v === undefined ? `{{${key}}}` : String(v);
  });
}
