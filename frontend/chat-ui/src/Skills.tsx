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
 * The pre-pivot scope input is gone — single-owner (DEC-033).
 *
 * State flow:
 *   catalog   (static, from GET /v1/skills/catalog)
 *   + installed (from GET /v1/skills/installed)
 *   → merged view in SkillCard (shows "Installed ✓" badge or Install button)
 */

import { useState, useEffect, useCallback } from 'react';
import { useNavigate } from 'react-router-dom';
import type {
  InstalledSkillPackResponse,
  SkillCatalogEntry,
  SkillKnobSchema,
} from '@xiaoguai/shared';
import { client } from './client';
import { useI18n } from './i18n/I18nProvider';
import { interpolate } from './i18n';

// ── tier (IA split) -----------------------------------------------------------

/** Catalog `tier` values. The server always supplies one, defaulting to
 *  `"specialized"` (DEC: general skills are surfaced first, scenarios behind a
 *  tab). Kept as string-literal constants so the toggle and filter agree. */
const TIER_GENERAL = 'general';
const TIER_SPECIALIZED = 'specialized';
type Tier = typeof TIER_GENERAL | typeof TIER_SPECIALIZED;

// ── bilingual rendering -------------------------------------------------------

/** First non-empty string, else `''`. Treats null / undefined / blank alike. */
function firstNonEmpty(...vals: Array<string | null | undefined>): string {
  for (const v of vals) {
    if (v != null && v.trim() !== '') return v;
  }
  return '';
}

/** Card name in the active locale: Chinese name under a zh locale (falling back
 *  to the English `name` when `name_zh` is null / empty), English otherwise. */
function localizedName(pack: SkillCatalogEntry, isZh: boolean): string {
  return isZh ? firstNonEmpty(pack.name_zh, pack.name) : pack.name;
}

/** Card description in the active locale; same fallback contract as
 *  {@link localizedName}. */
function localizedDesc(pack: SkillCatalogEntry, isZh: boolean): string {
  return isZh ? firstNonEmpty(pack.description_zh, pack.description) : pack.description;
}

// ── Phase 4c — pack-team activation -------------------------------------------

/**
 * True when an installed pack's agent team has been activated by the serve
 * boot-scan (Phase 4b flips `activation_status` from `"pending"` to
 * `"active"`). Read through a string coercion so this compiles regardless of
 * whether the shared `activation_status` union has been widened to include
 * `"active"` yet — the wire already carries it. Never throws on a null row.
 */
function isPackTeamActive(installed: InstalledSkillPackResponse | undefined): boolean {
  return (installed?.activation_status as string | undefined) === 'active';
}

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

/** localStorage key holding the chat-ui's last-active session id (set by
 *  App.tsx). The deep-link lands there so ExpertPicker has a session to attach
 *  the team to; absent → land on a fresh chat (`/`). */
const LAST_SESSION_KEY = 'xiaoguai.chat.lastSession';

/** Read the remembered last-active session id, tolerant of disabled storage. */
function lastSessionId(): string | null {
  try {
    return typeof localStorage !== 'undefined' ? localStorage.getItem(LAST_SESSION_KEY) : null;
  } catch {
    return null;
  }
}

interface SkillCardProps {
  pack: SkillCatalogEntry;
  installed: InstalledSkillPackResponse | undefined;
  /** True under a Chinese locale → prefer `name_zh` / `description_zh`. */
  isZh: boolean;
  onInstall: (pack: SkillCatalogEntry, config: Record<string, unknown>) => Promise<void>;
  onUninstall: (row: InstalledSkillPackResponse) => Promise<void>;
  /** Phase 4c — open chat with this pack's team pre-selected via `?team=`. */
  onUseInChat: (slug: string) => void;
}

function SkillCard({ pack, installed, isZh, onInstall, onUninstall, onUseInChat }: SkillCardProps) {
  const { t } = useI18n();
  const sp = t.ui.skills_page;
  const teamActive = isPackTeamActive(installed);
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
          <span className="skill-card__name">{localizedName(pack, isZh)}</span>
          <span className="skill-card__category">{pack.category}</span>
          <span className="skill-card__version">v{pack.version}</span>
          {installed && <span className="skill-card__badge">{sp.installed_badge}</span>}
          {/* Phase 4c — pack-team activation (Phase 4b flips this on boot). */}
          {teamActive && (
            <span
              className="skill-card__badge skill-card__badge--team"
              title={sp.team_active_title}
            >
              {sp.team_active_badge}
            </span>
          )}
        </div>
        <div className="skill-card__actions">
          {/* Phase 4c — deep-link to chat with this pack's team pre-selected. */}
          {teamActive && (
            <button
              className="skill-card__usechat-btn"
              onClick={() => onUseInChat(pack.slug)}
              title={sp.use_in_chat_title}
            >
              {sp.use_in_chat}
            </button>
          )}
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
      <p className="skill-card__desc">{localizedDesc(pack, isZh)}</p>

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
  const { t, locale } = useI18n();
  const navigate = useNavigate();
  const sp = t.ui.skills_page;
  const isZh = locale === 'zh-CN';
  const [catalog, setCatalog] = useState<SkillCatalogEntry[]>([]);
  const [installed, setInstalled] = useState<InstalledSkillPackResponse[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // IA split: general skills first (default tab), scenario packs behind a tab.
  const [selectedTier, setSelectedTier] = useState<Tier>(TIER_GENERAL);
  const { toasts, push: pushToast } = useToasts();

  // Restrict to the selected tier, then group that subset by category. Any tier
  // value other than "general" is treated as specialized so no pack is dropped.
  const tierPacks = catalog.filter((p) =>
    selectedTier === TIER_GENERAL ? p.tier === TIER_GENERAL : p.tier !== TIER_GENERAL,
  );
  const categories = Array.from(new Set(tierPacks.map((p) => p.category))).sort();

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
      pushToast(interpolate(sp.toast_installed, { name: localizedName(pack, isZh) }), 'success');
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

  /**
   * Phase 4c — "Use in chat": navigate to the last-active session (so the
   * ExpertPicker has a session to attach to) carrying `?team=<slug>`. With no
   * remembered session we land on a fresh chat — ChatPage then hints to send a
   * message first before the team can attach. The team itself lives in the
   * teams repo (Phase 4b boot-scan); ExpertPicker resolves the slug → team.
   */
  function handleUseInChat(slug: string) {
    const sid = lastSessionId();
    const query = `?team=${encodeURIComponent(slug)}`;
    navigate(sid ? `/sessions/${sid}${query}` : `/${query}`);
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
          {/* honest note: packs are templates — install records config only. */}
          <p className="skills-disclaimer">{sp.disclaimer}</p>
        </div>
      </div>

      {/* Phase 4c — feature intro + 3-step onboarding: a 专用场景 pack carries
          an agent team that, once active, runs complex tasks in the chat. */}
      <section className="skills-team-intro" aria-label={sp.team_intro_title}>
        <h2 className="skills-team-intro__title">{sp.team_intro_title}</h2>
        <p className="skills-team-intro__body">{sp.team_intro_body}</p>
        <div className="skills-team-intro__steps">
          <span className="skills-team-intro__steps-title">{sp.onboarding_title}</span>
          <ol className="skills-onboarding">
            <li>{sp.onboarding_step1}</li>
            <li>{sp.onboarding_step2}</li>
            <li>{sp.onboarding_step3}</li>
          </ol>
        </div>
      </section>

      {/* IA tier toggle (general vs specialized scenarios) */}
      <div className="skills-tiers" role="tablist" aria-label={sp.title}>
        <button
          type="button"
          role="tab"
          aria-selected={selectedTier === TIER_GENERAL}
          className={`skills-tier-tab${
            selectedTier === TIER_GENERAL ? ' skills-tier-tab--active' : ''
          }`}
          onClick={() => setSelectedTier(TIER_GENERAL)}
        >
          {sp.tab_general}
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={selectedTier === TIER_SPECIALIZED}
          className={`skills-tier-tab${
            selectedTier === TIER_SPECIALIZED ? ' skills-tier-tab--active' : ''
          }`}
          onClick={() => setSelectedTier(TIER_SPECIALIZED)}
        >
          {sp.tab_specialized}
        </button>
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
              {tierPacks
                .filter((p) => p.category === cat)
                .map((pack) => (
                  <SkillCard
                    key={pack.slug}
                    pack={pack}
                    installed={installedMap[pack.slug]}
                    isZh={isZh}
                    onInstall={handleInstall}
                    onUninstall={handleUninstall}
                    onUseInChat={handleUseInChat}
                  />
                ))}
            </div>
          </section>
        ))}
    </div>
  );
}
