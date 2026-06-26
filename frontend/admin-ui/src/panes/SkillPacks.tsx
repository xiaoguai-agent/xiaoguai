/**
 * v1.3.x — Skill Pack Browser pane.
 *
 * Surfaces these APIs:
 *   GET  /v1/skills/catalog       — list the static catalog baked into the binary
 *   GET  /v1/skills/installed     — list recorded packs
 *   POST /v1/skills/install       — record a new pack by ID
 *   POST /v1/admin/skills/rescan  — hot-activate installed packs' agent teams
 *
 * The operator can install in two ways:
 *   1. Pick a pre-built pack from the catalog dropdown (friendly path).
 *   2. Record an arbitrary pack id via the manual input (escape hatch).
 *
 * Packs with conversational agents are activated into a runnable agent team —
 * at `serve` boot (Phase 4b) or on demand via `POST /v1/admin/skills/rescan`
 * (Phase 5, the "Rescan & activate" button here). Such packs then show an
 * "Active" badge; packs with no conversational agents stay "Activation pending".
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
// IA tiers — catalog packs split into "general" (broadly-useful, shown first)
// and "specialized" (domain scenario packs, behind a tab). `tier` is always
// present on the catalog entry; anything that isn't "general" lives under the
// specialized tab.
// ---------------------------------------------------------------------------

type Tier = 'general' | 'specialized';

const TIER_GENERAL: Tier = 'general';
const TIER_SPECIALIZED: Tier = 'specialized';

function tierOf(entry: SkillCatalogEntry): Tier {
  return entry.tier === TIER_GENERAL ? TIER_GENERAL : TIER_SPECIALIZED;
}

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

type RescanState =
  | { kind: 'idle' }
  | { kind: 'running' }
  | { kind: 'done'; activated: string[] }
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
// Bilingual rendering — under a Chinese locale prefer the `*_zh` fields,
// falling back to the canonical English when null/empty. Mirrors the contract
// documented on `SkillCatalogEntry`.
// ---------------------------------------------------------------------------

function isChinese(locale: string): boolean {
  return locale.startsWith('zh');
}

/** Pick the locale-appropriate string, falling back when the localized value
 *  is null / empty / whitespace. */
function pickLocalized(
  primary: string | null | undefined,
  fallback: string,
  chinese: boolean,
): string {
  if (chinese && primary != null && primary.trim() !== '') return primary;
  return fallback;
}

function localizedName(entry: SkillCatalogEntry, chinese: boolean): string {
  return pickLocalized(entry.name_zh, entry.name, chinese);
}

function localizedDescription(entry: SkillCatalogEntry, chinese: boolean): string {
  return pickLocalized(entry.description_zh, entry.description, chinese);
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
          <span
            className={
              pack.activation_status === 'active' ? 'skill-badge-active' : 'skill-badge-pending'
            }
          >
            {pack.activation_status === 'active'
              ? t('pane.skill_packs.status_active')
              : t('pane.skill_packs.status_pending')}
          </span>

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
              {pack.activation_status === 'active' ? (
                <>
                  <span className="skill-badge-active">{t('pane.skill_packs.badge_active')}</span>
                  {' — '}
                  {t('pane.skill_packs.detail_activation_note_active')}
                </>
              ) : (
                <>
                  <span className="skill-badge-pending">{t('pane.skill_packs.badge_pending')}</span>
                  {' — '}
                  {t('pane.skill_packs.detail_activation_note')}
                </>
              )}
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
  tier: Tier;
  onTierChange: (tier: Tier) => void;
  onInstall: (slug: string) => void;
}

function CatalogInstall({
  catalog,
  installedSlugs,
  busy,
  tier,
  onTierChange,
  onInstall,
}: CatalogInstallProps): JSX.Element {
  const { t, i18n } = useTranslation();
  const chinese = isChinese(i18n.language);
  const [selected, setSelected] = useState('');

  // Only the active tier's packs are offered. Reset a stale selection when the
  // operator switches tiers so the install button never targets a hidden pack.
  const entries = catalog.kind === 'ok' ? catalog.entries.filter((e) => tierOf(e) === tier) : [];

  function handleTier(next: Tier) {
    if (next === tier) return;
    setSelected('');
    onTierChange(next);
  }

  const alreadyInstalled = selected !== '' && installedSlugs.has(selected);
  const canInstall = selected !== '' && !alreadyInstalled && !busy;

  return (
    <section className="skill-install-section">
      <h2 className="skill-install-heading">{t('pane.skill_packs.catalog_heading')}</h2>
      <p className="hint">{t('pane.skill_packs.catalog_hint')}</p>

      {/* IA tier tabs — general (default) vs specialized scenarios */}
      <div className="skill-tier-tabs" role="tablist" aria-label={t('pane.skill_packs.tier_tabs_aria')}>
        <button
          type="button"
          role="tab"
          aria-selected={tier === TIER_GENERAL}
          className={`skill-tier-tab${tier === TIER_GENERAL ? ' skill-tier-tab--active' : ''}`}
          onClick={() => handleTier(TIER_GENERAL)}
        >
          {t('pane.skill_packs.tier_general')}
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tier === TIER_SPECIALIZED}
          className={`skill-tier-tab${tier === TIER_SPECIALIZED ? ' skill-tier-tab--active' : ''}`}
          onClick={() => handleTier(TIER_SPECIALIZED)}
        >
          {t('pane.skill_packs.tier_specialized')}
        </button>
      </div>

      {catalog.kind === 'loading' && (
        <div className="empty">{t('pane.skill_packs.catalog_loading')}</div>
      )}

      {catalog.kind === 'error' && (
        <div className="error" role="alert">
          {t('pane.skill_packs.catalog_failed', { message: catalog.message })}
        </div>
      )}

      {catalog.kind === 'ok' && entries.length === 0 && (
        <div className="empty">{t('pane.skill_packs.catalog_empty')}</div>
      )}

      {catalog.kind === 'ok' && entries.length > 0 && (
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
            {entries.map((entry) => (
              <option key={entry.slug} value={entry.slug}>
                {localizedName(entry, chinese)} ({entry.category}) ·{' '}
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

      {/* Selected-pack description, localized to the active locale */}
      {catalog.kind === 'ok' &&
        selected !== '' &&
        (() => {
          const picked = entries.find((e) => e.slug === selected);
          const desc = picked ? localizedDescription(picked, chinese) : '';
          return desc !== '' ? <p className="skill-catalog-desc hint">{desc}</p> : null;
        })()}
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
  // Active IA tier — general packs are shown first.
  const [tier, setTier] = useState<Tier>(TIER_GENERAL);
  const [rescanState, setRescanState] = useState<RescanState>({ kind: 'idle' });
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

  async function handleRescan(): Promise<void> {
    setRescanState({ kind: 'running' });
    try {
      const resp = await client.rescanSkillPacks();
      setRescanState({ kind: 'done', activated: resp.activated });
      await reloadInstalled();
    } catch (err) {
      setRescanState({ kind: 'error', message: (err as Error).message });
    }
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

      {/* Honest disclaimer — these are templates; installing only records config */}
      <p className="hint skill-template-disclaimer">{t('pane.skill_packs.template_disclaimer')}</p>

      {/* Activation-pending notice — always visible */}
      <div className="skill-activation-notice" role="note">
        <strong>{t('pane.skill_packs.activation_notice_title')}</strong>{' '}
        {t('pane.skill_packs.activation_notice_body')}
      </div>

      <CatalogInstall
        catalog={catalogState}
        installedSlugs={installedSlugs}
        busy={catalogBusy}
        tier={tier}
        onTierChange={setTier}
        onInstall={(slug) => void handleCatalogInstall(slug)}
      />

      <InstallForm
        state={installState}
        onSubmit={handleInstallSubmit}
        onConfirm={() => void handleInstallConfirm()}
        onCancel={handleInstallCancel}
      />

      <section aria-label={t('pane.skill_packs.installed_heading')}>
        <div className="skill-installed-header">
          <h2 className="skill-section-heading">
            {t('pane.skill_packs.installed_heading')}
            {loadState.kind === 'ok' && <span className="skill-count">({packs.length})</span>}
          </h2>
          <button
            className="skill-install-btn"
            onClick={() => void handleRescan()}
            disabled={rescanState.kind === 'running'}
            title={t('pane.skill_packs.rescan_hint')}
          >
            {rescanState.kind === 'running'
              ? t('pane.skill_packs.rescan_running')
              : t('pane.skill_packs.rescan_btn')}
          </button>
        </div>

        {rescanState.kind === 'done' && (
          <div className="skill-install-done">
            ✓{' '}
            {rescanState.activated.length > 0
              ? t('pane.skill_packs.rescan_done', {
                  count: rescanState.activated.length,
                  slugs: rescanState.activated.join(', '),
                })
              : t('pane.skill_packs.rescan_noop')}
          </div>
        )}

        {rescanState.kind === 'error' && (
          <div className="error" role="alert">
            {t('pane.skill_packs.rescan_failed', { message: rescanState.message })}
          </div>
        )}

        {loadState.kind === 'loading' && (
          <div className="empty">{t('pane.skill_packs.installed_loading')}</div>
        )}

        {loadState.kind === 'error' && (
          <div className="error" role="alert">
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
                    {pack.activation_status === 'active' ? (
                      <span className="skill-badge-active">
                        {t('pane.skill_packs.status_active')}
                      </span>
                    ) : (
                      <span className="skill-badge-pending">
                        {t('pane.skill_packs.status_pending')}
                      </span>
                    )}
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
