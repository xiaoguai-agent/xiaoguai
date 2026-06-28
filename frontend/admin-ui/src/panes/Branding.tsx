/**
 * White-label branding pane — set the assistant's display name shown across the
 * chat UI (logo / welcome line / composer placeholder). Wraps GET/PUT
 * `/v1/branding`. DEC-033 single owner: one global name. An empty value means
 * the chat UI falls back to its built-in default ("Xiaoguai" / "小怪").
 *
 * Takes effect on the next chat-ui load (no live push) — that's the documented
 * behaviour; the demo reloads the chat tab to reveal the rename.
 */
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { client } from '../client';
import { ErrorBanner } from '../components/ErrorBanner';

const MAX_NAME_LEN = 64;

export function BrandingPane() {
  const { t } = useTranslation();
  const [name, setName] = useState('');
  const [loaded, setLoaded] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    client
      .getBranding()
      .then((b) => setName(b.assistant_name ?? ''))
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoaded(true));
  }, []);

  async function save() {
    setSaving(true);
    setSaved(false);
    setError(null);
    try {
      const b = await client.setBranding({ assistant_name: name.trim() });
      setName(b.assistant_name);
      setSaved(true);
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="pane">
      <h1>{t('pane.branding.title')}</h1>
      <p style={{ color: 'var(--muted, #888)', maxWidth: 560 }}>
        {t('pane.branding.intro')}
      </p>
      <ErrorBanner message={error ?? undefined} />
      <div style={{ display: 'flex', flexDirection: 'column', gap: 8, maxWidth: 480 }}>
        <label htmlFor="assistant-name">{t('pane.branding.name_label')}</label>
        <input
          id="assistant-name"
          type="text"
          value={name}
          maxLength={MAX_NAME_LEN}
          placeholder={t('pane.branding.name_placeholder')}
          disabled={!loaded || saving}
          onChange={(e) => {
            setName(e.target.value);
            setSaved(false);
          }}
        />
        <p style={{ color: 'var(--muted, #888)', fontSize: 13, margin: 0 }}>
          {t('pane.branding.hint')}
        </p>
        <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
          <button onClick={() => void save()} disabled={!loaded || saving}>
            {saving ? t('pane.branding.saving') : t('pane.branding.save')}
          </button>
          {saved && (
            <span style={{ color: 'var(--ok, #2e7d32)' }}>✓ {t('pane.branding.saved')}</span>
          )}
        </div>
      </div>
    </div>
  );
}
