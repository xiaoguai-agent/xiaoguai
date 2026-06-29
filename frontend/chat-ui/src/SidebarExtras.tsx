/**
 * SidebarExtras — small presentational widgets that enrich the chat-ui left
 * sidebar (Feature ② / ⑤). Kept deliberately dumb: the data-fetching lives in
 * App.tsx and is passed down as props, so these stay easy to reason about and
 * never block the sidebar render on a network call.
 *
 *   - <TodayTokenStat>   a muted "今日 ~X tokens" line (humanized K/M).
 *   - <WorkingDirControl> compact editor for the active session's working_dir.
 */

import { useEffect, useState } from 'react';
import { useI18n } from './i18n/I18nProvider';

/**
 * Humanize a token count: 1_234 → "1.2K", 4_500_000 → "4.5M". Sub-1000 counts
 * render verbatim. Returns "0" for null/NaN/negative so the caller can still
 * show a stable line.
 */
export function humanizeTokens(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n) || n < 0) return '0';
  if (n < 1000) return String(Math.round(n));
  if (n < 1_000_000) return `${(n / 1000).toFixed(1).replace(/\.0$/, '')}K`;
  return `${(n / 1_000_000).toFixed(1).replace(/\.0$/, '')}M`;
}

interface TodayTokenStatProps {
  /** Total input + output tokens used today, or `null` while loading / on error. */
  total: number | null;
  /** True while the fetch is in flight (first load) — render a skeleton. */
  loading: boolean;
}

/**
 * A muted one-liner showing today's token spend. Renders a skeleton while
 * loading and hides entirely if the fetch failed (total stays `null` and
 * loading is false) — the sidebar never breaks on a usage outage.
 */
export function TodayTokenStat({ total, loading }: TodayTokenStatProps) {
  const { t } = useI18n();
  if (loading) {
    return <p className="sidebar-token-stat sidebar-token-stat--skeleton" aria-hidden="true">…</p>;
  }
  if (total == null) return null;
  return (
    <p className="sidebar-token-stat" title={t.sidebar.today_tokens_title}>
      <span aria-hidden="true">📊 </span>
      {t.sidebar.today_tokens.replace('{{count}}', humanizeTokens(total))}
    </p>
  );
}

interface WorkingDirControlProps {
  /** The active session id, or undefined when not viewing a session. */
  sessionId: string | undefined;
  /** Current stored working_dir for the active session (undefined = unset). */
  value: string | undefined;
  /** Persist a new value (empty string clears the override). Returns a promise
   *  that resolves once the PATCH lands; rejects to surface an error. */
  onSave: (sessionId: string, workingDir: string) => Promise<void>;
}

/**
 * Compact inline editor for the active session's coding workspace root
 * (Feature ⑤). A plain text input — browsers cannot pick a server-side path —
 * plus a small save button. Empty input clears the per-session override.
 * Renders nothing when no session is active.
 */
export function WorkingDirControl({ sessionId, value, onSave }: WorkingDirControlProps) {
  const { t } = useI18n();
  const [draft, setDraft] = useState<string>(value ?? '');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Re-sync the draft when the active session (or its stored value) changes,
  // unless the user is mid-save. Immutable: a fresh string per render input.
  useEffect(() => {
    setDraft(value ?? '');
    setError(null);
  }, [sessionId, value]);

  if (!sessionId) return null;

  const dirty = draft.trim() !== (value ?? '').trim();
  // Coding tools (governed file read/edit, Feature ⑤) activate the moment a
  // session has a non-empty stored working_dir — surface that as a muted note
  // tied to the saved value (not the in-progress draft).
  const codingActive = (value ?? '').trim().length > 0;

  async function handleSave() {
    if (!sessionId || saving) return;
    setSaving(true);
    setError(null);
    try {
      await onSave(sessionId, draft.trim());
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="working-dir" aria-label={t.sidebar.working_dir}>
      <label className="working-dir__label" htmlFor="working-dir-input">
        <span aria-hidden="true">📁 </span>
        {t.sidebar.working_dir}
      </label>
      <div className="working-dir__row">
        <input
          id="working-dir-input"
          className="working-dir__input"
          type="text"
          value={draft}
          placeholder={t.sidebar.working_dir_placeholder}
          spellCheck={false}
          autoComplete="off"
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              void handleSave();
            }
          }}
        />
        <button
          type="button"
          className="working-dir__save"
          disabled={saving || !dirty}
          onClick={() => void handleSave()}
          title={t.sidebar.working_dir_save}
          aria-label={t.sidebar.working_dir_save}
        >
          {saving ? '…' : t.sidebar.working_dir_save}
        </button>
      </div>
      {codingActive && (
        <p className="working-dir__coding-note" title={t.sidebar.coding_active_title}>
          <span aria-hidden="true">🛡 </span>
          {t.sidebar.coding_active}
        </p>
      )}
      {error && (
        <p className="working-dir__error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
