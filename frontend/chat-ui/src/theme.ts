/**
 * v0.8.2 theme switcher.
 *
 * Three settings live in `localStorage["xiaoguai-theme"]`:
 *   "light" | "dark" | "system"
 *
 * "system" follows `prefers-color-scheme` and updates when the OS pref
 * changes mid-session. The effective theme is applied to
 * `document.documentElement` via `data-theme="light" | "dark"`, which the
 * CSS token fork in styles.css picks up.
 */

import { useEffect, useState } from 'react';

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

/** Hook returning the current choice + a setter that persists + applies it. */
export function useTheme(): {
  choice: ThemeChoice;
  effective: EffectiveTheme;
  setChoice: (next: ThemeChoice) => void;
} {
  const [choice, setChoiceState] = useState<ThemeChoice>(readChoice);
  const [eff, setEff] = useState<EffectiveTheme>(() => effective(readChoice()));

  // Apply on every choice change.
  useEffect(() => {
    const next = effective(choice);
    setEff(next);
    apply(next);
  }, [choice]);

  // When the user picked "system", listen for OS preference changes so the
  // page recolours without a manual toggle.
  useEffect(() => {
    if (choice !== 'system' || typeof window === 'undefined' || !window.matchMedia) return;
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    const onChange = () => {
      const next: EffectiveTheme = mq.matches ? 'dark' : 'light';
      setEff(next);
      apply(next);
    };
    mq.addEventListener('change', onChange);
    return () => mq.removeEventListener('change', onChange);
  }, [choice]);

  function setChoice(next: ThemeChoice): void {
    window.localStorage.setItem(KEY, next);
    setChoiceState(next);
  }

  return { choice, effective: eff, setChoice };
}

/**
 * Module-level side effect: apply the persisted theme as early as
 * possible to avoid a flash of unstyled (light) content while React
 * mounts. Safe to import for its side effect from `main.tsx`.
 */
export function applyInitialTheme(): void {
  apply(effective(readChoice()));
}
