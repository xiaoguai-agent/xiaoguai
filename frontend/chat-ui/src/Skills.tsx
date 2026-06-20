/**
 * Skills pane — v1.2.28 (DEC-041 consolidation).
 *
 * Lists available skill packs from the catalog, shows install state, and lets
 * the operator install / uninstall packs. Knob configuration is JSON-schema–
 * driven: the catalog carries `knobs` metadata and the pane renders a typed
 * form (int/number slider, bool toggle, string / enum select).
 *
 * DEC-041: now goes through the typed shared `XiaoguaiClient` instead of raw
 * `fetch`, so requests carry owner auth (the raw fetches dropped the
 * Authorization header — a 401 when auth is enabled) and use the canonical
 * catalog/installed wire types. This also fixes the prior installed-detection
 * bug: `GET /v1/skills/installed` returns `pack_id`, but the old code keyed its
 * map by `pack_slug` (absent → always undefined → never showed "Installed ✓").
 * The per-tenant input is gone — single-owner (DEC-033) ignores tenant.
 *
 * State flow:
 *   catalog   (static, from GET /v1/skills/catalog)
 *   + installed (from GET /v1/skills/installed)
 *   → merged view in SkillCard (shows "Installed ✓" badge or Install button)
 */

import { useState, useEffect, useCallback } from 'react';
import type {
  InstalledSkillPackResponse,
  SkillCatalogEntry,
  SkillKnobSchema,
} from '@xiaoguai/shared';
import { client } from './client';
import { useI18n } from './i18n/I18nProvider';
import { interpolate } from './i18n';

// ── toast notification --------------------------------------------------------

interface Toast {
  id: number;
  message: string;
  kind: 'success' | 'error';
}

function useToasts() {
  const [toasts, setToasts] = useState<Toast[]>([]);
  let nextId = 0;

  const push = useCallback((message: string, kind: Toast['kind']) => {
    const id = ++nextId;
    setToasts((prev) => [...prev, { id, message, kind }]);
    setTimeout(() => setToasts((prev) => prev.filter((t) => t.id !== id)), 3500);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  return { toasts, push };
}

// ── knob form ----------------------------------------------------------------

function defaultConfig(knobs: Record<string, SkillKnobSchema>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [key, schema] of Object.entries(knobs)) {
    out[key] = schema.default;
  }
  return out;
}

interface KnobFormProps {
  knobs: Record<string, SkillKnobSchema>;
  values: Record<string, unknown>;
  onChange: (values: Record<string, unknown>) => void;
}

function KnobForm({ knobs, values, onChange }: KnobFormProps) {
  if (Object.keys(knobs).length === 0) return null;

  function set(key: string, value: unknown) {
    onChange({ ...values, [key]: value });
  }

  return (
    <div className="knob-form">
      {Object.entries(knobs).map(([key, schema]) => (
        <div key={key} className="knob-row">
          <label className="knob-label" title={schema.description}>
            {key}
          </label>
          {schema.type === 'boolean' ? (
            <input
              type="checkbox"
              checked={Boolean(values[key] ?? schema.default)}
              onChange={(e) => set(key, e.target.checked)}
            />
          ) : schema.type === 'integer' || schema.type === 'number' ? (
            <input
              type="number"
              step={schema.type === 'number' ? 'any' : 1}
              value={Number(values[key] ?? schema.default)}
              onChange={(e) => set(key, Number(e.target.value))}
              className="knob-input"
            />
          ) : schema.enum && schema.enum.length > 0 ? (
            <select
              value={String(values[key] ?? schema.default)}
              onChange={(e) => set(key, e.target.value)}
              className="knob-select"
            >
              {schema.enum.map((opt) => (
                <option key={opt} value={opt}>
                  {opt}
                </option>
              ))}
            </select>
          ) : (
            <input
              type="text"
              value={String(values[key] ?? schema.default)}
              onChange={(e) => set(key, e.target.value)}
              className="knob-input"
            />
          )}
          <span className="knob-desc">{schema.description}</span>
        </div>
      ))}
    </div>
  );
}

// ── skill card ----------------------------------------------------------------

interface SkillCardProps {
  pack: SkillCatalogEntry;
  installed: InstalledSkillPackResponse | undefined;
  onInstall: (pack: SkillCatalogEntry, config: Record<string, unknown>) => Promise<void>;
  onUninstall: (row: InstalledSkillPackResponse) => Promise<void>;
}

function SkillCard({ pack, installed, onInstall, onUninstall }: SkillCardProps) {
  const { t } = useI18n();
  const sp = t.ui.skills_page;
  const knobs = pack.knobs ?? {};
  const featureFlags = pack.requires?.feature_flags ?? [];
  const envKeys = pack.requires?.env_keys ?? [];
  const [expanded, setExpanded] = useState(false);
  const [config, setConfig] = useState<Record<string, unknown>>(() => defaultConfig(knobs));
  const [busy, setBusy] = useState(false);

  async function handleInstall() {
    setBusy(true);
    try {
      await onInstall(pack, config);
    } finally {
      setBusy(false);
    }
  }

  async function handleUninstall() {
    if (!installed) return;
    setBusy(true);
    try {
      await onUninstall(installed);
    } finally {
      setBusy(false);
    }
  }

  const hasKnobs = Object.keys(knobs).length > 0;

  return (
    <div className={`skill-card${installed ? ' skill-card--installed' : ''}`}>
      {/* summary row */}
      <div className="skill-card__header">
        <div className="skill-card__meta">
          <span className="skill-card__name">{pack.name}</span>
          <span className="skill-card__category">{pack.category}</span>
          <span className="skill-card__version">v{pack.version}</span>
          {installed && <span className="skill-card__badge">{sp.installed_badge}</span>}
        </div>
        <div className="skill-card__actions">
          {hasKnobs && (
            <button
              className="skill-card__detail-btn"
              onClick={() => setExpanded((x) => !x)}
              aria-expanded={expanded}
            >
              {expanded ? sp.less : sp.configure}
            </button>
          )}
          {installed ? (
            <button
              className="skill-card__uninstall-btn"
              onClick={handleUninstall}
              disabled={busy}
            >
              {busy ? sp.busy : sp.uninstall}
            </button>
          ) : (
            <button
              className="skill-card__install-btn"
              onClick={handleInstall}
              disabled={busy}
            >
              {busy ? sp.busy : sp.install}
            </button>
          )}
        </div>
      </div>

      {/* description */}
      <p className="skill-card__desc">{pack.description}</p>

      {/* prerequisite tags */}
      {(featureFlags.length > 0 || envKeys.length > 0) && (
        <div className="skill-card__requires">
          {featureFlags.map((f) => (
            <span key={f} className="skill-tag skill-tag--flag">
              {interpolate(sp.requires_flag, { flag: f })}
            </span>
          ))}
          {envKeys.map((e) => (
            <span key={e} className="skill-tag skill-tag--env">
              {interpolate(sp.requires_env, { env: e })}
            </span>
          ))}
        </div>
      )}

      {/* expandable knob configurator */}
      {expanded && hasKnobs && (
        <KnobForm knobs={knobs} values={config} onChange={setConfig} />
      )}
    </div>
  );
}

// ── main pane ----------------------------------------------------------------

export function SkillsPage() {
  const { t } = useI18n();
  const sp = t.ui.skills_page;
  const [catalog, setCatalog] = useState<SkillCatalogEntry[]>([]);
  const [installed, setInstalled] = useState<InstalledSkillPackResponse[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const { toasts, push: pushToast } = useToasts();

  // Group packs by category for rendering.
  const categories = Array.from(new Set(catalog.map((p) => p.category))).sort();

  // Installed lookup keyed by pack_id (== catalog slug server-side). The old
  // code keyed by `pack_slug`, which the API response doesn't carry — so the
  // "Installed ✓" badge never showed. DEC-041 fix.
  const installedMap = Object.fromEntries(installed.map((r) => [r.pack_id, r]));

  const reload = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [cat, inst] = await Promise.all([
        client.listSkillCatalog(),
        client.listInstalledSkillPacks(),
      ]);
      setCatalog(cat.packs);
      setInstalled(inst);
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  async function handleInstall(pack: SkillCatalogEntry, config: Record<string, unknown>) {
    try {
      await client.installSkillPack({ pack_id: pack.slug, config });
      // Re-fetch so the row carries the full installed shape (the install
      // response is a narrower projection).
      setInstalled(await client.listInstalledSkillPacks());
      pushToast(interpolate(sp.toast_installed, { name: pack.name }), 'success');
    } catch (err) {
      pushToast(interpolate(sp.toast_install_failed, { message: (err as Error).message }), 'error');
    }
  }

  async function handleUninstall(row: InstalledSkillPackResponse) {
    try {
      await client.uninstallSkillPack(row.id);
      setInstalled((prev) => prev.filter((r) => r.id !== row.id));
      pushToast(interpolate(sp.toast_uninstalled, { name: row.name }), 'success');
    } catch (err) {
      pushToast(
        interpolate(sp.toast_uninstall_failed, { message: (err as Error).message }),
        'error',
      );
    }
  }

  return (
    <div className="skills-page">
      {/* toast stack */}
      <div className="toast-stack" aria-live="polite">
        {toasts.map((t) => (
          <div key={t.id} className={`toast toast--${t.kind}`}>
            {t.message}
          </div>
        ))}
      </div>

      {/* header */}
      <div className="skills-header">
        <div>
          <h1 className="skills-title">{sp.title}</h1>
          <p className="skills-subtitle">{sp.subtitle}</p>
        </div>
      </div>

      {/* body */}
      {loading && <p className="skills-status">{sp.loading}</p>}
      {error && (
        <p className="skills-status skills-status--error" role="alert">
          {interpolate(sp.error, { message: error })}
        </p>
      )}

      {!loading && !error && categories.length === 0 && (
        <p className="skills-status">{sp.empty}</p>
      )}

      {!loading &&
        !error &&
        categories.map((cat) => (
          <section key={cat} className="skills-category">
            <h2 className="skills-category-title">{cat}</h2>
            <div className="skills-grid">
              {catalog
                .filter((p) => p.category === cat)
                .map((pack) => (
                  <SkillCard
                    key={pack.slug}
                    pack={pack}
                    installed={installedMap[pack.slug]}
                    onInstall={handleInstall}
                    onUninstall={handleUninstall}
                  />
                ))}
            </div>
          </section>
        ))}
    </div>
  );
}
