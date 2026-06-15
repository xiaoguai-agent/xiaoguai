/**
 * v1.3.x — Skill Pack Browser pane.
 *
 * Surfaces three APIs:
 *   GET  /v1/skills/catalog    — list the static catalog baked into the binary
 *   GET  /v1/skills/installed  — list recorded packs
 *   POST /v1/skills/install    — record a new pack by ID
 *
 * The operator can install in two ways:
 *   1. Pick a pre-built pack from the catalog dropdown (friendly path).
 *   2. Record an arbitrary pack id via the manual input (escape hatch).
 *
 * Runtime loader activation is NOT yet wired server-side.  All installed
 * packs show an "Activation pending" badge and a prose notice is rendered
 * at the top of the page so operators are not misled.
 */

import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type {
  InstalledSkillPackResponse,
  SkillCatalogEntry,
} from '@xiaoguai/shared';
import { client } from '../client';
import { PaneIntro } from '../components/PaneIntro';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type LoadState =
  | { kind: 'loading' }
  | { kind: 'ok'; packs: InstalledSkillPackResponse[] }
  | { kind: 'error'; message: string };

type CatalogState =
  | { kind: 'loading' }
  | { kind: 'ok'; entries: SkillCatalogEntry[] }
  | { kind: 'error'; message: string };

type InstallState =
  | { kind: 'idle' }
  | { kind: 'confirming'; packId: string }
  | { kind: 'installing' }
  | { kind: 'done'; name: string }
  | { kind: 'error'; message: string };

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function fmtDate(iso: string): string {
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  return d.toLocaleString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function commaSep(items: string[]): string {
  return items.length === 0 ? '—' : items.join(', ');
}

// ---------------------------------------------------------------------------
// Detail drawer
// ---------------------------------------------------------------------------

interface DetailDrawerProps {
  pack: InstalledSkillPackResponse;
  onClose: () => void;
}

function DetailDrawer({ pack, onClose }: DetailDrawerProps): JSX.Element {
  const { t } = useTranslation();
  // Close on Escape key
  const closeRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('keydown', handler);
    closeRef.current?.focus();
    return () => document.removeEventListener('keydown', handler);
  }, [onClose]);

  return (
    <aside
      className="skill-drawer"
      role="complementary"
      aria-label={t('pane.skill_packs.drawer_aria')}
    >
      <div className="skill-drawer-overlay" onClick={onClose} aria-hidden="true" />
      <div className="skill-drawer-panel">
        <header className="skill-drawer-header">
          <div>
            <h2 className="skill-drawer-title">{pack.name}</h2>
            <span className="skill-drawer-pack-id">{pack.pack_id}</span>
          </div>
          <button
            ref={closeRef}
            className="skill-drawer-close"
            onClick={onClose}
            aria-label={t('pane.skill_packs.drawer_close')}
          >
            ✕
          </button>
        </header>

        <div className="skill-drawer-body">
          <span className="skill-badge-pending">{t('pane.skill_packs.status_pending')}</span>

          {pack.description && <p className="skill-drawer-desc">{pack.description}</p>}

          <dl className="skill-drawer-dl">
            <dt>{t('pane.skill_packs.detail_version')}</dt>
            <dd>
              <code>{pack.version}</code>
            </dd>

            <dt>{t('pane.skill_packs.detail_recorded_at')}</dt>
            <dd>{fmtDate(pack.recorded_at)}</dd>

            <dt>{t('pane.skill_packs.detail_agents')}</dt>
            <dd>{commaSep(pack.agents)}</dd>

            <dt>{t('pane.skill_packs.detail_inbound')}</dt>
            <dd>{commaSep(pack.inbound_adapters)}</dd>

            <dt>{t('pane.skill_packs.detail_outputs')}</dt>
            <dd>{commaSep(pack.outputs)}</dd>

            <dt>{t('pane.skill_packs.detail_activation_status')}</dt>
            <dd>
              <span className="skill-badge-pending">{t('pane.skill_packs.badge_pending')}</span>
              {' — '}
              {t('pane.skill_packs.detail_activation_note')}
            </dd>
          </dl>
        </div>
      </div>
    </aside>
  );
}

// ---------------------------------------------------------------------------
// Catalog install — pick a pre-built pack from the catalog
// ---------------------------------------------------------------------------

interface CatalogInstallProps {
  catalog: CatalogState;
  installedSlugs: Set<string>;
  busy: boolean;
  onInstall: (slug: string) => void;
}

function CatalogInstall({
  catalog,
  installedSlugs,
  busy,
  onInstall,
}: CatalogInstallProps): JSX.Element {
  const { t } = useTranslation();
  const [selected, setSelected] = useState('');

  const alreadyInstalled = selected !== '' && installedSlugs.has(selected);
  const canInstall = selected !== '' && !alreadyInstalled && !busy;

  return (
    <section className="skill-install-section">
      <h2 className="skill-install-heading">{t('pane.skill_packs.catalog_heading')}</h2>
      <p className="hint">{t('pane.skill_packs.catalog_hint')}</p>

      {catalog.kind === 'loading' && (
        <div className="empty">{t('pane.skill_packs.catalog_loading')}</div>
      )}

      {catalog.kind === 'error' && (
        <div className="error">
          {t('pane.skill_packs.catalog_failed', { message: catalog.message })}
        </div>
      )}

      {catalog.kind === 'ok' && catalog.entries.length === 0 && (
        <div className="empty">{t('pane.skill_packs.catalog_empty')}</div>
      )}

      {catalog.kind === 'ok' && catalog.entries.length > 0 && (
        <div className="skill-catalog-form">
          <label className="skill-catalog-label" htmlFor="skill-catalog-select">
            {t('pane.skill_packs.catalog_select_label')}
          </label>
          <select
            id="skill-catalog-select"
            className="skill-catalog-select"
            value={selected}
            onChange={(e) => setSelected(e.target.value)}
            disabled={busy}
          >
            <option value="">{t('pane.skill_packs.catalog_select_placeholder')}</option>
            {catalog.entries.map((entry) => (
              <option key={entry.slug} value={entry.slug}>
                {entry.name} ({entry.category}) ·{' '}
                {t('pane.skill_packs.catalog_version', { version: entry.version })}
                {installedSlugs.has(entry.slug)
                  ? ` — ${t('pane.skill_packs.catalog_already_installed')}`
                  : ''}
              </option>
            ))}
          </select>
          <button
            className="skill-install-btn"
            onClick={() => onInstall(selected)}
            disabled={!canInstall}
          >
            {busy
              ? t('pane.skill_packs.catalog_btn_installing')
              : alreadyInstalled
                ? t('pane.skill_packs.catalog_already_installed')
                : t('pane.skill_packs.catalog_btn_install')}
          </button>
        </div>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------
// Manual install form (escape hatch — record an arbitrary pack id)
// ---------------------------------------------------------------------------

interface InstallFormProps {
  state: InstallState;
  onSubmit: (packId: string) => void;
  onConfirm: () => void;
  onCancel: () => void;
}

function InstallForm({ state, onSubmit, onConfirm, onCancel }: InstallFormProps): JSX.Element {
  const { t } = useTranslation();
  const [packId, setPackId] = useState('');

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = packId.trim();
    if (!trimmed) return;
    onSubmit(trimmed);
  }

  const locked = state.kind === 'installing' || state.kind === 'confirming';

  return (
    <section className="skill-install-section">
      <h2 className="skill-install-heading">{t('pane.skill_packs.manual_heading')}</h2>
      <form onSubmit={handleSubmit} className="skill-install-form">
        <input
          className="skill-install-input"
          type="text"
          placeholder={t('pane.skill_packs.manual_placeholder')}
          value={packId}
          onChange={(e) => setPackId(e.target.value)}
          disabled={locked}
          aria-label={t('pane.skill_packs.col_pack_id')}
        />
        <button type="submit" className="skill-install-btn" disabled={!packId.trim() || locked}>
          {state.kind === 'installing'
            ? t('pane.skill_packs.manual_btn_recording')
            : t('pane.skill_packs.manual_btn_install')}
        </button>
      </form>

      {/* Confirmation dialog */}
      {state.kind === 'confirming' && (
        <div
          className="skill-confirm"
          role="dialog"
          aria-modal="true"
          aria-label={t('pane.skill_packs.confirm_confirm')}
        >
          <p>
            <strong>{t('pane.skill_packs.confirm_record', { packId: state.packId })}</strong>{' '}
            <em>{t('pane.skill_packs.confirm_note')}</em>
          </p>
          <div className="skill-confirm-actions">
            <button onClick={onConfirm} className="skill-install-btn">
              {t('pane.skill_packs.confirm_confirm')}
            </button>
            <button onClick={onCancel} className="skill-cancel-btn">
              {t('pane.skill_packs.confirm_cancel')}
            </button>
          </div>
        </div>
      )}

      {state.kind === 'done' && (
        <div className="skill-install-done">
          ✓ {t('pane.skill_packs.installed_done', { name: state.name })}{' '}
          <span className="skill-badge-pending">{t('pane.skill_packs.badge_pending')}</span>
        </div>
      )}

      {state.kind === 'error' && <div className="error skill-install-error">{state.message}</div>}
    </section>
  );
}

// ---------------------------------------------------------------------------
// Main pane
// ---------------------------------------------------------------------------

export function SkillPacksPane(): JSX.Element {
  const { t } = useTranslation();
  const [loadState, setLoadState] = useState<LoadState>({ kind: 'loading' });
  const [catalogState, setCatalogState] = useState<CatalogState>({ kind: 'loading' });
  const [installState, setInstallState] = useState<InstallState>({ kind: 'idle' });
  const [catalogBusy, setCatalogBusy] = useState(false);
  const [selected, setSelected] = useState<InstalledSkillPackResponse | null>(null);
  // Remember packId across confirm/install cycle
  const pendingPackId = useRef<string>('');

  async function reloadInstalled(): Promise<void> {
    try {
      const packs = await client.listInstalledSkillPacks();
      setLoadState({ kind: 'ok', packs });
    } catch (err) {
      setLoadState({ kind: 'error', message: (err as Error).message });
    }
  }

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const packs = await client.listInstalledSkillPacks();
        if (!cancelled) setLoadState({ kind: 'ok', packs });
      } catch (err) {
        if (!cancelled) setLoadState({ kind: 'error', message: (err as Error).message });
      }
    })();
    void (async () => {
      try {
        const resp = await client.listSkillCatalog();
        if (!cancelled) setCatalogState({ kind: 'ok', entries: resp.packs });
      } catch (err) {
        if (!cancelled) setCatalogState({ kind: 'error', message: (err as Error).message });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Slugs already recorded — used to disable re-installing the same pack.
  const installedSlugs = new Set(
    loadState.kind === 'ok' ? loadState.packs.map((p) => p.pack_id) : [],
  );

  async function handleCatalogInstall(slug: string): Promise<void> {
    setCatalogBusy(true);
    try {
      await client.installSkillPack({ pack_id: slug });
      await reloadInstalled();
    } catch {
      // Surface failures via the installed-list reload / next attempt; the
      // dropdown stays usable so the operator can retry.
      await reloadInstalled();
    } finally {
      setCatalogBusy(false);
    }
  }

  function handleInstallSubmit(packId: string) {
    pendingPackId.current = packId;
    setInstallState({ kind: 'confirming', packId });
  }

  async function handleInstallConfirm() {
    const packId = pendingPackId.current;
    setInstallState({ kind: 'installing' });
    try {
      const resp = await client.installSkillPack({ pack_id: packId });
      setInstallState({ kind: 'done', name: resp.name });
      await reloadInstalled();
    } catch (err) {
      setInstallState({ kind: 'error', message: (err as Error).message });
    }
  }

  function handleInstallCancel() {
    setInstallState({ kind: 'idle' });
  }

  const packs = loadState.kind === 'ok' ? loadState.packs : [];

  return (
    <>
      <h1>{t('pane.skill_packs.title')}</h1>
      <PaneIntro
        purpose={t('pane.skill_packs.intro.purpose')}
        usage={t('pane.skill_packs.intro.usage')}
        usageLabel={t('pane.skill_packs.intro.usage_label')}
      />

      {/* Activation-pending notice — always visible */}
      <div className="skill-activation-notice" role="note">
        <strong>{t('pane.skill_packs.activation_notice_title')}</strong>{' '}
        {t('pane.skill_packs.activation_notice_body')}
      </div>

      <CatalogInstall
        catalog={catalogState}
        installedSlugs={installedSlugs}
        busy={catalogBusy}
        onInstall={(slug) => void handleCatalogInstall(slug)}
      />

      <InstallForm
        state={installState}
        onSubmit={handleInstallSubmit}
        onConfirm={() => void handleInstallConfirm()}
        onCancel={handleInstallCancel}
      />

      <section aria-label={t('pane.skill_packs.installed_heading')}>
        <h2 className="skill-section-heading">
          {t('pane.skill_packs.installed_heading')}
          {loadState.kind === 'ok' && <span className="skill-count">({packs.length})</span>}
        </h2>

        {loadState.kind === 'loading' && (
          <div className="empty">{t('pane.skill_packs.installed_loading')}</div>
        )}

        {loadState.kind === 'error' && (
          <div className="error">
            {t('pane.skill_packs.installed_failed', { message: loadState.message })}
          </div>
        )}

        {loadState.kind === 'ok' && packs.length === 0 && (
          <div className="empty">{t('pane.skill_packs.installed_empty')}</div>
        )}

        {loadState.kind === 'ok' && packs.length > 0 && (
          <table>
            <thead>
              <tr>
                <th>{t('pane.skill_packs.col_name')}</th>
                <th>{t('pane.skill_packs.col_pack_id')}</th>
                <th>{t('pane.skill_packs.col_version')}</th>
                <th>{t('pane.skill_packs.col_agents')}</th>
                <th>{t('pane.skill_packs.col_status')}</th>
                <th>{t('pane.skill_packs.col_recorded_at')}</th>
                <th aria-label="Actions" />
              </tr>
            </thead>
            <tbody>
              {packs.map((pack) => (
                <tr key={pack.id}>
                  <td>{pack.name}</td>
                  <td>
                    <code>{pack.pack_id}</code>
                  </td>
                  <td>
                    <span className="tag">{pack.version}</span>
                  </td>
                  <td>
                    {pack.agents.length === 0 ? (
                      <em style={{ color: 'var(--muted)', fontSize: '12px' }}>
                        {t('pane.skill_packs.agents_none')}
                      </em>
                    ) : (
                      pack.agents.join(', ')
                    )}
                  </td>
                  <td>
                    <span className="skill-badge-pending">
                      {t('pane.skill_packs.status_pending')}
                    </span>
                  </td>
                  <td style={{ fontSize: '12px', color: 'var(--muted)' }}>
                    {fmtDate(pack.recorded_at)}
                  </td>
                  <td>
                    <button
                      className="skill-detail-btn"
                      onClick={() => setSelected(pack)}
                      aria-label={t('pane.skill_packs.details_aria', { name: pack.name })}
                    >
                      {t('pane.skill_packs.btn_details')}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>

      {selected !== null && <DetailDrawer pack={selected} onClose={() => setSelected(null)} />}
    </>
  );
}
