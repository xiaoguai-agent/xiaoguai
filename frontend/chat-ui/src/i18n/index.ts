/**
 * Minimal i18n helper — resolves the browser locale to one of the bundled
 * translation files without pulling in a full i18next dependency.
 *
 * Supported locales: en (default), zh-CN.
 * Falls back to en for any locale not in the map.
 */

import en from './locales/en/translation.json';
import zhCN from './locales/zh-CN/translation.json';

export type Locale = 'en' | 'zh-CN';

/** Locales offered in the language switcher, with native display labels. */
export const AVAILABLE_LOCALES: ReadonlyArray<{ code: Locale; label: string }> = [
  { code: 'en', label: 'English' },
  { code: 'zh-CN', label: '中文' },
];

/** localStorage key holding the operator's explicit language choice. */
const LOCALE_STORAGE_KEY = 'xiaoguai.locale';

interface TranslationShape {
  /** Main chat-ui surface strings (sidebar, welcome, composer). */
  ui: {
    new_chat: string;
    skills: string;
    admin: string;
    language_label: string;
    no_sessions: string;
    /** Phase 2 (Cherry-Studio IA) — narrow icon nav-rail labels. */
    nav: {
      chat: string;
      skills: string;
      activity: string;
      providers: string;
      usage: string;
      incidents: string;
      loops: string;
      memory: string;
      hotl: string;
      branding: string;
      mcp: string;
      anomaly: string;
      scheduler: string;
      settings: string;
    };
    /** Phase 2 (Cherry-Studio IA) — assistant/topic list-panel strings. */
    assistant: {
      tab_assistants: string;
      tab_topics: string;
      general: string;
      /** Fixed role line for the pinned 通用 row (no persona). */
      general_desc: string;
      group_personas: string;
      group_teams: string;
      search_placeholder: string;
      empty: string;
      error_load: string;
      /** Locked expert row: "Install first: {{items}}". */
      locked_hint: string;
      /** Locked expert row CTA → the Skills page. */
      locked_cta: string;
      /** Locked expert row tooltip. */
      locked_title: string;
    };
    /** Phase 3 (Cherry-Studio IA) — chat-area top bar (model selector). */
    header: {
      model_label: string;
      /** Muted hint after the active-assistant name ("via 助手"). */
      active_assistant_hint: string;
    };
    /** Default assistant display name when no white-label branding is set. */
    assistant_name: string;
    welcome_title: string;
    welcome_subtitle: string;
    composer_placeholder: string;
    composer_hint: string;
    stop: string;
    send: string;
    stop_generating: string;
    send_message: string;
    thinking: string;
    /** Phase 4b — per-message hover action toolbar (copy / regenerate / edit /
     *  branch / delete) + their status / confirm strings. */
    message_actions: {
      toolbar_label: string;
      copy: string;
      regenerate: string;
      edit: string;
      branch: string;
      delete: string;
      delete_confirm: string;
      copied: string;
      copy_failed: string;
      delete_failed: string;
      regenerate_failed: string;
      edit_failed: string;
      edit_prompt: string;
    };
    /** Opening prompts on the welcome screen (Gemini-style chips). */
    suggestions: {
      summarize_doc: string;
      write_shell: string;
      analyze_cve: string;
      explain_codebase: string;
    };
    /** Sidebar session-row actions (rename / delete + confirm prompt). */
    session: {
      rename: string;
      delete: string;
      delete_confirm: string;
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
      /** Phase 4c — first-run hint explaining where teams come from. */
      firstrun_hint: string;
      /** Phase 4c — deep-link landing: team auto-attached / needs a session. */
      deeplink_attached: string;
      deeplink_need_session: string;
      /** Feature ⑭ — Team Run discoverability hint (shown when a team is active). */
      teamrun_discover: string;
      error_load: string;
      error_attach: string;
      error_detach: string;
      error_suggest: string;
    };
    /** T5.2 — consult/execute mode toggle by the send box. */
    mode: {
      toggle_label: string;
      execute: string;
      consult: string;
      execute_hint: string;
      readonly_cue: string;
    };
    /** T5.2 — team parallel run entry (T4 orchestrate, deferred UI). */
    teamrun: {
      button: string;
      button_title: string;
      disabled_consult: string;
      started: string;
      progress: string;
      synthesizing: string;
      done: string;
      failed: string;
      error: string;
      /** Phase 4c — seeded example goal hint under the team-run entry. */
      example_goal: string;
    };
    /** Skills pane (skill-pack catalog browser + install/uninstall). */
    skills_page: {
      title: string;
      subtitle: string;
      /** Feature ④ — ability-center reframe (installed-first consumer view). */
      center_subtitle: string;
      active_title: string;
      active_empty: string;
      browse_title: string;
      browse_hint: string;
      loading: string;
      error: string;
      empty: string;
      install: string;
      uninstall: string;
      configure: string;
      less: string;
      busy: string;
      installed_badge: string;
      requires_flag: string;
      requires_env: string;
      toast_installed: string;
      toast_install_failed: string;
      toast_uninstalled: string;
      toast_uninstall_failed: string;
      /** IA tier tabs (general vs specialized scenarios). */
      tab_general: string;
      tab_specialized: string;
      /** Honest note: packs are templates; running needs data source / MCP. */
      disclaimer: string;
      /** Phase 4c — feature intro: scenario packs carry an agent team. */
      team_intro_title: string;
      team_intro_body: string;
      /** Phase 4c — activation badge + deep-link on an active pack card. */
      team_active_badge: string;
      team_active_title: string;
      use_in_chat: string;
      use_in_chat_title: string;
      /** Phase 4c — inline 3-step onboarding (install → active → use). */
      onboarding_title: string;
      onboarding_step1: string;
      onboarding_step2: string;
      onboarding_step3: string;
    };
    /** Feature ④ — read-only "tools available to this session" (MCP) view. */
    mcp_tools: {
      title: string;
      intro: string;
      manage_note: string;
      loading: string;
      error: string;
      empty: string;
      transport_title: string;
      env_label: string;
    };
    /** VM-ops welcome card — lists the VMware MCP skills + one-click install. */
    vmware_starter: {
      title: string;
      subtitle: string;
      install: string;
      installed: string;
      installing: string;
      install_failed: string;
      badge_readonly: string;
      badge_ops: string;
      prereq_note: string;
      more_in_marketplace: string;
      chip_label: string;
      loading: string;
      error: string;
    };
  };
  /** Feature ②/⑤ — enriched sidebar widgets (audit link, token stat,
   *  per-session working-dir control). */
  sidebar: {
    /** Audit / activity deep-link label → /admin/audit. */
    audit: string;
    /** "今日 ~{{count}} tokens" — `{{count}}` is the humanized total. */
    today_tokens: string;
    /** Tooltip for the token stat line. */
    today_tokens_title: string;
    /** Working-dir control label. */
    working_dir: string;
    /** Working-dir save button. */
    working_dir_save: string;
    /** Working-dir input placeholder (an example absolute path). */
    working_dir_placeholder: string;
    /** Feature ⑤ — prominent CTA button shown when no working_dir is set yet
     *  (collapsed state); clicking it reveals + focuses the path input. */
    working_dir_set_cta: string;
    /** Feature ⑤ — small "change / edit" affordance on the set-dir chip,
     *  re-opening the input. */
    working_dir_edit: string;
    /** Tooltip for the edit affordance on the set-dir chip. */
    working_dir_edit_title: string;
    /** Feature ⑤ — muted note shown when a working_dir is set: governed
     *  file read/edit is active for the session, scoped to that directory. */
    coding_active: string;
    /** Tooltip expanding on the coding-active note (execute writes / consult previews). */
    coding_active_title: string;
  };
  chat: {
    /** Feature ⑥ — non-blocking "a turn is still running server-side" cue. */
    remote_running: string;
    /** Feature ⑥ — tooltip explaining the remote-running indicator. */
    remote_running_title: string;
    sse: {
      reconnecting: string;
      cancel_reconnect: string;
      gave_up: string;
      load_failed: string;
      stream_error: string;
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
};

function detectLocale(): Locale {
  const nav = typeof navigator !== 'undefined' ? navigator.language : 'en';
  if (nav === 'zh-CN' || nav.startsWith('zh')) return 'zh-CN';
  return 'en';
}

/** True when `value` is one of the bundled locales. */
function isLocale(value: string | null): value is Locale {
  return value === 'en' || value === 'zh-CN';
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
