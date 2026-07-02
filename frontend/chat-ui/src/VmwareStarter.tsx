/**
 * VmwareStarter — a friendly one-click starter card for the VM-ops assistant.
 *
 * Surfaced in ChatPage's welcome state when the active assistant is the
 * VMware-ops persona (or via the "🖥 VMware 运维" welcome chip). It (a) lists
 * the VMware MCP skills the assistant can drive and (b) lets the operator
 * install the servers they need with one click.
 *
 * Data comes from the live marketplace catalog (`listMarketplace`), filtered to
 * the `vmware-*` slugs — names are never hardcoded here. Already-installed
 * servers (matched by name against `listMcpServers`) pre-mark their button as
 * done. Localized name/description follow the `name_zh` / `description_zh`
 * fallback contract used elsewhere in the chat-ui.
 *
 * HONEST prerequisite: installing here only registers the server in xiaoguai.
 * It runs only once the operator has `uv tool install`ed the package on the
 * host AND configured the vCenter connection. The card states this plainly.
 */
import { useCallback, useEffect, useState } from 'react';
import type { MarketplaceEntry } from '@xiaoguai/shared';
import { client } from './client';
import { useI18n } from './i18n/I18nProvider';
import { interpolate } from './i18n';

/** Slug prefix for the VMware marketplace family. */
const VMWARE_PREFIX = 'vmware-';
/** Slugs that get a special leading position + an explanatory badge. */
const SLUG_MONITOR = 'vmware-monitor';
const SLUG_AIOPS = 'vmware-aiops';
/** Guide path surfaced in the prerequisite note (mentioned, not linked). */

/** Per-row install lifecycle. */
type RowStatus =
  | { kind: 'idle' }
  | { kind: 'installing' }
  | { kind: 'installed' }
  | { kind: 'failed'; message: string };

/** First non-empty string, else `''` (null / undefined / blank all skipped). */
function firstNonEmpty(...vals: Array<string | null | undefined>): string {
  for (const v of vals) {
    if (v != null && v.trim() !== '') return v;
  }
  return '';
}

/** Localized display name with the standard `name_zh` → `name` fallback. */
function localizedName(entry: MarketplaceEntry, isZh: boolean): string {
  return isZh ? firstNonEmpty(entry.name_zh, entry.name) : entry.name;
}

/** Localized description with the standard `description_zh` → `description` fallback. */
function localizedDesc(entry: MarketplaceEntry, isZh: boolean): string {
  return isZh
    ? firstNonEmpty(entry.description_zh, entry.description)
    : entry.description;
}

/**
 * Order the VMware entries: monitor first (read-only, recommended), then aiops
 * (operations), then the rest in catalog order. Pure — returns a new array.
 */
function orderVmwareEntries(entries: MarketplaceEntry[]): MarketplaceEntry[] {
  const rank = (slug: string): number => {
    if (slug === SLUG_MONITOR) return 0;
    if (slug === SLUG_AIOPS) return 1;
    return 2;
  };
  return [...entries].sort((a, b) => rank(a.slug) - rank(b.slug));
}

export function VmwareStarter() {
  const { t, locale } = useI18n();
  const vs = t.ui.vmware_starter;
  const isZh = locale === 'zh-CN';

  const [entries, setEntries] = useState<MarketplaceEntry[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  // Per-slug install state. Slugs not present default to { kind: 'idle' }.
  const [statuses, setStatuses] = useState<Record<string, RowStatus>>({});

  const statusFor = (slug: string): RowStatus => statuses[slug] ?? { kind: 'idle' };

  // Immutable status update for one slug.
  const setStatus = useCallback((slug: string, next: RowStatus) => {
    setStatuses((prev) => ({ ...prev, [slug]: next }));
  }, []);

  // On mount: load the marketplace catalog + the installed servers in parallel.
  // The vmware entries drive the list; the installed servers pre-mark rows
  // (matched by name — install creates the mcp_servers row from the entry, so
  // the entry `name` equals the server `name`). Both are best-effort: a catalog
  // failure surfaces an inline error; an installed-servers failure just leaves
  // every row as idle (the install call is still safe to attempt).
  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const [market, servers] = await Promise.all([
          client.listMarketplace(),
          client.listMcpServers().catch(() => []),
        ]);
        if (!alive) return;
        const vmware = orderVmwareEntries(
          market.entries.filter((e) => e.slug.startsWith(VMWARE_PREFIX)),
        );
        setEntries(vmware);
        const installedNames = new Set(servers.map((s) => s.name));
        const preset: Record<string, RowStatus> = {};
        for (const e of vmware) {
          if (installedNames.has(e.name)) preset[e.slug] = { kind: 'installed' };
        }
        setStatuses(preset);
        setLoadError(null);
      } catch (err) {
        if (alive) setLoadError((err as Error).message);
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  // One-click install. Idempotent at the UI level: a row already installing or
  // installed is not re-triggered. On success the row flips to "installed ✓";
  // on failure it surfaces the message and returns to an actionable state.
  const install = useCallback(
    async (slug: string) => {
      const cur = statuses[slug] ?? { kind: 'idle' };
      if (cur.kind === 'installing' || cur.kind === 'installed') return;
      setStatus(slug, { kind: 'installing' });
      try {
        await client.installMarketplace({ slug });
        setStatus(slug, { kind: 'installed' });
      } catch (err) {
        setStatus(slug, { kind: 'failed', message: (err as Error).message });
      }
    },
    [statuses, setStatus],
  );

  if (loadError !== null) {
    return (
      <div className="vmware-starter" data-testid="vmware-starter">
        <p className="vmware-starter__error">
          {interpolate(vs.error, { message: loadError })}
        </p>
      </div>
    );
  }

  if (entries === null) {
    return (
      <div className="vmware-starter" data-testid="vmware-starter">
        <p className="vmware-starter__loading">{vs.loading}</p>
      </div>
    );
  }

  return (
    <div className="vmware-starter" data-testid="vmware-starter">
      <div className="vmware-starter__head">
        <h2 className="vmware-starter__title">{vs.title}</h2>
        <p className="vmware-starter__subtitle">{vs.subtitle}</p>
      </div>

      <ul className="vmware-starter__list">
        {entries.map((entry) => {
          const status = statusFor(entry.slug);
          const installed = status.kind === 'installed';
          const installing = status.kind === 'installing';
          const badge =
            entry.slug === SLUG_MONITOR
              ? vs.badge_readonly
              : entry.slug === SLUG_AIOPS
                ? vs.badge_ops
                : null;
          const buttonLabel = installed
            ? vs.installed
            : installing
              ? vs.installing
              : vs.install;
          return (
            <li key={entry.slug} className="vmware-starter__row">
              <div className="vmware-starter__info">
                <div className="vmware-starter__name-line">
                  <span className="vmware-starter__name">
                    {localizedName(entry, isZh)}
                  </span>
                  {badge && (
                    <span
                      className={`vmware-starter__badge${
                        entry.slug === SLUG_MONITOR
                          ? ' vmware-starter__badge--readonly'
                          : ' vmware-starter__badge--ops'
                      }`}
                    >
                      {badge}
                    </span>
                  )}
                </div>
                <p className="vmware-starter__desc">{localizedDesc(entry, isZh)}</p>
                {status.kind === 'failed' && (
                  <p className="vmware-starter__row-error">
                    {interpolate(vs.install_failed, { message: status.message })}
                  </p>
                )}
              </div>
              <button
                type="button"
                className={`vmware-starter__install${
                  installed ? ' vmware-starter__install--done' : ''
                }`}
                onClick={() => void install(entry.slug)}
                disabled={installed || installing}
                data-testid={`vmware-install-${entry.slug}`}
              >
                {buttonLabel}
              </button>
            </li>
          );
        })}
      </ul>

      <p className="vmware-starter__more">{vs.more_in_marketplace}</p>
      <p className="vmware-starter__prereq">{vs.prereq_note}</p>
    </div>
  );
}
