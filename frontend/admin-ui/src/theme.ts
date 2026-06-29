/**
 * Shared theme bootstrap for admin-ui.
 *
 * The theme *choice* is owned by chat-ui's toggle (the rail switch) and
 * persisted to `localStorage["xiaoguai-theme"]` as one of:
 *   "light" | "dark" | "system"
 *
 * Admin-ui has no toggle of its own — it simply READS that same key on load
 * and applies the effective theme to `document.documentElement` via
 * `data-theme="light" | "dark"`, which the CSS token fork in styles.css picks
 * up. This keeps light/dark consistent when navigating chat → /admin/.
 *
 * "system" follows `prefers-color-scheme` and recolours when the OS pref
 * changes mid-session. Logic mirrors chat-ui's `theme.ts` exactly.
 */

export type ThemeChoice = 'light' | 'dark' | 'system';
export type EffectiveTheme = 'light' | 'dark';

const KEY = 'xiaoguai-theme';
const DEFAULT: ThemeChoice = 'system';

function readChoice(): ThemeChoice {
  if (typeof window === 'undefined') return DEFAULT;
  const v = window.localStorage.getItem(KEY);
  return v === 'light' || v === 'dark' || v === 'system' ? v : DEFAULT;
}

function systemTheme(): EffectiveTheme {
  if (typeof window === 'undefined' || !window.matchMedia) return 'light';
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

function effective(choice: ThemeChoice): EffectiveTheme {
  return choice === 'system' ? systemTheme() : choice;
}

function apply(theme: EffectiveTheme): void {
  if (typeof document === 'undefined') return;
  document.documentElement.setAttribute('data-theme', theme);
}

/**
 * Apply the persisted theme as early as possible (call from `main.tsx` before
 * React mounts) and, when the choice is "system", keep the page in sync with
 * OS preference changes for the life of the session.
 *
 * Safe under jsdom: `window.matchMedia` is feature-detected before use.
 */
export function applyInitialTheme(): void {
  const choice = readChoice();
  apply(effective(choice));

  if (choice !== 'system' || typeof window === 'undefined' || !window.matchMedia) return;
  const mq = window.matchMedia('(prefers-color-scheme: dark)');
  const onChange = (): void => apply(mq.matches ? 'dark' : 'light');
  mq.addEventListener('change', onChange);
}
