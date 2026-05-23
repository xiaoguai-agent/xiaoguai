/**
 * v0.8.2 theme toggle — a small three-way segmented switch
 * (light / system / dark). Lives in the sidebar footer.
 */

import { useTheme, type ThemeChoice } from './theme';

const OPTIONS: Array<{ value: ThemeChoice; label: string; title: string }> = [
  { value: 'light', label: '☀', title: 'Light theme' },
  { value: 'system', label: '◐', title: 'Follow system preference' },
  { value: 'dark', label: '☾', title: 'Dark theme' },
];

export function ThemeToggle() {
  const { choice, setChoice } = useTheme();

  return (
    <div className="theme-toggle" role="radiogroup" aria-label="Theme">
      {OPTIONS.map((opt) => (
        <button
          key={opt.value}
          type="button"
          role="radio"
          aria-checked={choice === opt.value}
          className={`theme-toggle__btn${choice === opt.value ? ' active' : ''}`}
          title={opt.title}
          onClick={() => setChoice(opt.value)}
        >
          <span aria-hidden="true">{opt.label}</span>
        </button>
      ))}
    </div>
  );
}
