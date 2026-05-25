/**
 * Skills pane — v1.2.28
 *
 * Lists available skill packs from the catalog, shows install state, and lets
 * the operator install / uninstall packs for a tenant. Knob configuration is
 * JSON-schema–driven: the catalog carries `knobs` metadata and the pane
 * renders a typed form (int slider, bool toggle, string / enum select).
 *
 * State flow:
 *   catalog (static, from GET /v1/skills/catalog)
 *   + installed (per-tenant, from GET /v1/skills/installed?tenant=...)
 *   → merged view in SkillCard (shows "Installed ✓" badge or Install button)
 */

import { useState, useEffect, useCallback } from 'react';
import { client } from './client';

// ── wire types ----------------------------------------------------------------

interface PackRequires {
  feature_flags: string[];
  env_keys: string[];
}

type KnobSchema =
  | { type: 'integer'; default: number; description: string }
  | { type: 'boolean'; default: boolean; description: string }
  | { type: 'string'; enum: string[]; default: string; description: string };

interface SkillPackEntry {
  slug: string;
  name: string;
  description: string;
  version: string;
  category: string;
  requires: PackRequires;
  knobs: Record<string, KnobSchema>;
  screenshot_url?: string | null;
}

interface CatalogResponse {
  version: number;
  packs: SkillPackEntry[];
}

interface InstalledPackRow {
  id: string;
  tenant_id: string;
  pack_slug: string;
  version: string;
  config: Record<string, unknown>;
  installed_at: string;
}

// ── minimal API helpers -------------------------------------------------------

async function fetchCatalog(): Promise<CatalogResponse> {
  const resp = await fetch(`${(client as unknown as { baseUrl: string }).baseUrl}/v1/skills/catalog`);
  if (!resp.ok) throw new Error(`catalog: HTTP ${resp.status}`);
  return (await resp.json()) as CatalogResponse;
}

async function fetchInstalled(tenantId: string): Promise<InstalledPackRow[]> {
  const resp = await fetch(
    `${(client as unknown as { baseUrl: string }).baseUrl}/v1/skills/installed?tenant=${encodeURIComponent(tenantId)}`,
  );
  if (!resp.ok) throw new Error(`installed: HTTP ${resp.status}`);
  return (await resp.json()) as InstalledPackRow[];
}

async function apiInstall(
  tenantId: string,
  packSlug: string,
  config: Record<string, unknown>,
): Promise<InstalledPackRow> {
  const resp = await fetch(
    `${(client as unknown as { baseUrl: string }).baseUrl}/v1/skills/install`,
    {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ tenant_id: tenantId, pack_slug: packSlug, config }),
    },
  );
  if (!resp.ok) {
    const body = (await resp.json().catch(() => ({}))) as { message?: string };
    throw new Error(body.message ?? `install: HTTP ${resp.status}`);
  }
  return (await resp.json()) as InstalledPackRow;
}

async function apiUninstall(id: string): Promise<void> {
  const baseUrl = (client as unknown as { baseUrl: string }).baseUrl;
  const resp = await fetch(`${baseUrl}/v1/skills/install/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });
  if (!resp.ok) throw new Error(`uninstall: HTTP ${resp.status}`);
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

function defaultConfig(knobs: Record<string, KnobSchema>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [key, schema] of Object.entries(knobs)) {
    out[key] = schema.default;
  }
  return out;
}

interface KnobFormProps {
  knobs: Record<string, KnobSchema>;
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
          ) : schema.type === 'integer' ? (
            <input
              type="number"
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
  pack: SkillPackEntry;
  installed: InstalledPackRow | undefined;
  onInstall: (pack: SkillPackEntry, config: Record<string, unknown>) => Promise<void>;
  onUninstall: (row: InstalledPackRow) => Promise<void>;
}

function SkillCard({ pack, installed, onInstall, onUninstall }: SkillCardProps) {
  const [expanded, setExpanded] = useState(false);
  const [config, setConfig] = useState<Record<string, unknown>>(() => defaultConfig(pack.knobs));
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

  const hasKnobs = Object.keys(pack.knobs).length > 0;

  return (
    <div className={`skill-card${installed ? ' skill-card--installed' : ''}`}>
      {/* summary row */}
      <div className="skill-card__header">
        <div className="skill-card__meta">
          <span className="skill-card__name">{pack.name}</span>
          <span className="skill-card__category">{pack.category}</span>
          <span className="skill-card__version">v{pack.version}</span>
          {installed && <span className="skill-card__badge">Installed ✓</span>}
        </div>
        <div className="skill-card__actions">
          {hasKnobs && (
            <button
              className="skill-card__detail-btn"
              onClick={() => setExpanded((x) => !x)}
              aria-expanded={expanded}
            >
              {expanded ? 'Less' : 'Configure'}
            </button>
          )}
          {installed ? (
            <button
              className="skill-card__uninstall-btn"
              onClick={handleUninstall}
              disabled={busy}
            >
              {busy ? '…' : 'Uninstall'}
            </button>
          ) : (
            <button
              className="skill-card__install-btn"
              onClick={handleInstall}
              disabled={busy}
            >
              {busy ? '…' : 'Install'}
            </button>
          )}
        </div>
      </div>

      {/* description */}
      <p className="skill-card__desc">{pack.description}</p>

      {/* prerequisite tags */}
      {(pack.requires.feature_flags.length > 0 || pack.requires.env_keys.length > 0) && (
        <div className="skill-card__requires">
          {pack.requires.feature_flags.map((f) => (
            <span key={f} className="skill-tag skill-tag--flag">
              flag: {f}
            </span>
          ))}
          {pack.requires.env_keys.map((e) => (
            <span key={e} className="skill-tag skill-tag--env">
              env: {e}
            </span>
          ))}
        </div>
      )}

      {/* expandable knob configurator */}
      {expanded && hasKnobs && (
        <KnobForm knobs={pack.knobs} values={config} onChange={setConfig} />
      )}
    </div>
  );
}

// ── main pane ----------------------------------------------------------------

const DEFAULT_TENANT = 'default';

export function SkillsPage() {
  const [catalog, setCatalog] = useState<SkillPackEntry[]>([]);
  const [installed, setInstalled] = useState<InstalledPackRow[]>([]);
  const [tenantId, setTenantId] = useState(DEFAULT_TENANT);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const { toasts, push: pushToast } = useToasts();

  // Group packs by category for rendering.
  const categories = Array.from(new Set(catalog.map((p) => p.category))).sort();

  const installedMap = Object.fromEntries(installed.map((r) => [r.pack_slug, r]));

  // Fetch catalog once; fetch installed whenever tenantId changes.
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    Promise.all([fetchCatalog(), fetchInstalled(tenantId)])
      .then(([cat, inst]) => {
        if (!cancelled) {
          setCatalog(cat.packs);
          setInstalled(inst);
        }
      })
      .catch((err: Error) => {
        if (!cancelled) setError(err.message);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [tenantId]);

  async function handleInstall(pack: SkillPackEntry, config: Record<string, unknown>) {
    try {
      const row = await apiInstall(tenantId, pack.slug, config);
      setInstalled((prev) => [...prev, row]);
      pushToast(`${pack.name} installed`, 'success');
    } catch (err) {
      pushToast(`Install failed: ${(err as Error).message}`, 'error');
    }
  }

  async function handleUninstall(row: InstalledPackRow) {
    const pack = catalog.find((p) => p.slug === row.pack_slug);
    try {
      await apiUninstall(row.id);
      setInstalled((prev) => prev.filter((r) => r.id !== row.id));
      pushToast(`${pack?.name ?? row.pack_slug} uninstalled`, 'success');
    } catch (err) {
      pushToast(`Uninstall failed: ${(err as Error).message}`, 'error');
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
          <h1 className="skills-title">Skill Packs</h1>
          <p className="skills-subtitle">
            Browse and install pre-built skill packs for your workspace. Packs extend the
            agent with domain-specific tools, prompts, and RAG indexes.
          </p>
        </div>
        <div className="skills-tenant-control">
          <label htmlFor="tenant-input" className="skills-tenant-label">
            Tenant
          </label>
          <input
            id="tenant-input"
            className="skills-tenant-input"
            value={tenantId}
            onChange={(e) => setTenantId(e.target.value.trim() || DEFAULT_TENANT)}
            placeholder="tenant id"
          />
        </div>
      </div>

      {/* body */}
      {loading && <p className="skills-status">Loading…</p>}
      {error && <p className="skills-status skills-status--error">Error: {error}</p>}

      {!loading && !error && categories.length === 0 && (
        <p className="skills-status">No skill packs available.</p>
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
