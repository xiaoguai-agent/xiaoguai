/**
 * v0.9.4 — MCP marketplace pane.
 *
 * Lists the static catalog shipped with `xiaoguai-api`, grouped by
 * category. Each row has an Install button that POSTs the slug to
 * `/v1/mcp/marketplace/install`, writing an `mcp_servers` row. We
 * don't refetch the live `mcp_servers` list after install; users
 * navigate to the "MCP Servers" pane to confirm.
 *
 * Roadmap principle: curation, not hosting. Catalog ships in-repo;
 * users audit a single JSON file.
 */

import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { safeHref } from '@xiaoguai/shared';
import type { MarketplaceEntry } from '@xiaoguai/shared';
import { client } from '../client';

type Status =
  | { kind: 'idle' }
  | { kind: 'installing'; slug: string }
  | { kind: 'installed'; slug: string }
  | { kind: 'error'; slug: string; message: string };

export function MarketplacePane() {
  const { t } = useTranslation();
  const [entries, setEntries] = useState<MarketplaceEntry[] | null>(null);
  const [version, setVersion] = useState<number | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [status, setStatus] = useState<Status>({ kind: 'idle' });

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const r = await client.listMarketplace();
        if (!cancelled) {
          setEntries(r.entries);
          setVersion(r.version);
        }
      } catch (err) {
        if (!cancelled) setLoadError((err as Error).message);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const grouped = useMemo(() => {
    if (!entries) return null;
    const map = new Map<string, MarketplaceEntry[]>();
    for (const e of entries) {
      const list = map.get(e.category) ?? [];
      list.push(e);
      map.set(e.category, list);
    }
    return [...map.entries()].sort(([a], [b]) => a.localeCompare(b));
  }, [entries]);

  async function install(slug: string): Promise<void> {
    setStatus({ kind: 'installing', slug });
    try {
      await client.installMarketplace({ slug });
      setStatus({ kind: 'installed', slug });
    } catch (err) {
      setStatus({ kind: 'error', slug, message: (err as Error).message });
    }
  }

  return (
    <>
      <h1>{t('pane.marketplace.title')}</h1>
      <p className="hint">
        {t('pane.marketplace.description', {
          version: version === null ? '…' : String(version),
        })}
      </p>

      {loadError && <div className="error">{t('common.failed', { message: loadError })}</div>}

      {grouped === null ? (
        <div className="empty">{t('pane.marketplace.empty_loading')}</div>
      ) : (
        grouped.map(([category, items]) => (
          <section key={category} className="marketplace-section">
            <h2>{category}</h2>
            <div className="marketplace-grid">
              {items.map((entry) => {
                // SEC-25: source_url comes from the backend-served catalog —
                // only whitelisted schemes become a link; otherwise omit it.
                const sourceHref = safeHref(entry.source_url);
                return (
                <article key={entry.slug} className="marketplace-card">
                  <header>
                    <h3>{entry.name}</h3>
                    <span className="tag">{entry.transport}</span>
                  </header>
                  <p>{entry.description}</p>
                  {entry.env_keys && entry.env_keys.length > 0 && (
                    <div className="env-keys">
                      {t('pane.marketplace.required_env')}{' '}
                      {entry.env_keys.map((k) => (
                        <code key={k}>{k}</code>
                      ))}
                    </div>
                  )}
                  <footer>
                    {sourceHref && (
                      <a
                        href={sourceHref}
                        target="_blank"
                        rel="noreferrer noopener"
                      >
                        {t('pane.marketplace.source_link')}
                      </a>
                    )}
                    <InstallButton
                      slug={entry.slug}
                      status={status}
                      onClick={() => void install(entry.slug)}
                    />
                  </footer>
                </article>
                );
              })}
            </div>
          </section>
        ))
      )}
    </>
  );
}

function InstallButton({
  slug,
  status,
  onClick,
}: {
  slug: string;
  status: Status;
  onClick: () => void;
}) {
  const { t } = useTranslation();
  const label = (() => {
    if (status.kind === 'installing' && status.slug === slug) return t('pane.marketplace.btn_installing');
    if (status.kind === 'installed' && status.slug === slug) return t('pane.marketplace.btn_installed');
    if (status.kind === 'error' && status.slug === slug) return t('pane.marketplace.btn_retry');
    return t('pane.marketplace.btn_install');
  })();
  const disabled = status.kind === 'installing' && status.slug === slug;
  return (
    <div className="install-action">
      <button onClick={onClick} disabled={disabled}>
        {label}
      </button>
      {status.kind === 'error' && status.slug === slug && (
        <span className="error-msg">{status.message}</span>
      )}
    </div>
  );
}
