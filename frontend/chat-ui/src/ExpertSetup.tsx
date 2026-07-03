/**
 * ExpertSetup — the per-expert prerequisite checklist (v1.34).
 *
 * Reached from a locked expert row's 「去安装」 CTA (`/experts/:key`). It shows
 * exactly what an expert needs and lets the operator satisfy it:
 *   - required OR-groups (at least one item each) + the optional add-ons;
 *   - `mcp` items install inline (one click → `installMarketplace` → readiness
 *     refetch → the ✓ / the whole expert flips to ready);
 *   - `package` items can't be installed from the browser (they're host
 *     `uv tool install`s), so they show the exact command + a copy button and
 *     an honest probe status (✓ ready / ✗ not found).
 *
 * Fail-safe: a load error shows inline; nothing here can hallucinate an
 * install — the ✓ comes only from the server's live readiness.
 */
import { useCallback, useEffect, useState } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import type { ExpertItemReadiness, ExpertReadiness } from '@xiaoguai/shared';
import { client } from './client';
import { useI18n } from './i18n/I18nProvider';
import { interpolate } from './i18n';

type RowStatus =
  | { kind: 'idle' }
  | { kind: 'installing' }
  | { kind: 'failed'; message: string };

export function ExpertSetup() {
  const { key } = useParams<{ key: string }>();
  const { t, locale } = useI18n();
  const es = t.ui.expert_setup;
  const isZh = locale === 'zh-CN';
  const navigate = useNavigate();

  const [expert, setExpert] = useState<ExpertReadiness | null>(null);
  const [offlineHint, setOfflineHint] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [statuses, setStatuses] = useState<Record<string, RowStatus>>({});
  const [copied, setCopied] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const resp = await client.listExperts();
      const found = resp.experts.find((e) => e.key === key) ?? null;
      setExpert(found);
      setOfflineHint(isZh ? resp.offline_hint : (resp.offline_hint_en ?? resp.offline_hint));
      setLoadError(found ? null : es.unknown);
    } catch (err) {
      setLoadError((err as Error).message);
    } finally {
      setLoading(false);
    }
  }, [key, isZh, es.unknown]);

  useEffect(() => {
    void load();
  }, [load]);

  const installMcp = useCallback(
    async (slug: string) => {
      setStatuses((p) => ({ ...p, [slug]: { kind: 'installing' } }));
      try {
        await client.installMarketplace({ slug });
        setStatuses((p) => ({ ...p, [slug]: { kind: 'idle' } }));
        await load(); // refetch readiness so the ✓ (and maybe `ready`) updates.
      } catch (err) {
        setStatuses((p) => ({
          ...p,
          [slug]: { kind: 'failed', message: (err as Error).message },
        }));
      }
    },
    [load],
  );

  const copy = useCallback((text: string) => {
    void navigator.clipboard?.writeText(text);
    setCopied(text);
    setTimeout(() => setCopied(null), 1500);
  }, []);

  if (loading) {
    return <div className="expert-setup">{es.loading}</div>;
  }
  if (loadError || !expert) {
    return (
      <div className="expert-setup">
        <button type="button" className="expert-setup__back" onClick={() => navigate('/')}>
          {es.back}
        </button>
        <p className="expert-setup__error">{loadError ?? es.unknown}</p>
      </div>
    );
  }

  const title = isZh ? (expert.name_zh ?? expert.name) : expert.name;
  const summary = isZh ? (expert.summary_zh ?? expert.summary) : expert.summary;

  return (
    <div className="expert-setup">
      <button type="button" className="expert-setup__back" onClick={() => navigate('/')}>
        {es.back}
      </button>

      <div className="expert-setup__head">
        <h1 className="expert-setup__title">{title}</h1>
        <span
          className={`expert-setup__badge${expert.ready ? ' ready' : ''}`}
          title={expert.ready ? es.ready_badge : es.not_ready_badge}
        >
          {expert.ready ? `✓ ${es.ready_badge}` : `🔒 ${es.not_ready_badge}`}
        </span>
      </div>
      {summary && <p className="expert-setup__summary">{summary}</p>}

      <h2 className="expert-setup__section">{es.required_title}</h2>
      {expert.required.map((g, gi) => (
        <div key={gi} className={`expert-setup__group${g.satisfied ? ' satisfied' : ''}`}>
          <div className="expert-setup__group-label">
            {(isZh ? (g.label_zh ?? g.label) : g.label)}
            {g.any_of.length > 1 && <span className="expert-setup__anyof"> · {es.any_of}</span>}
            {g.satisfied && <span className="expert-setup__group-ok"> ✓</span>}
          </div>
          {g.any_of.map((item) => (
            <ItemRow
              key={item.id}
              item={item}
              status={statuses[item.id] ?? { kind: 'idle' }}
              copied={copied}
              labels={es}
              onInstall={() => void installMcp(item.id)}
              onCopy={copy}
            />
          ))}
        </div>
      ))}

      {expert.optional.length > 0 && (
        <>
          <h2 className="expert-setup__section">{es.optional_title}</h2>
          <p className="expert-setup__optional-note">{es.optional_note}</p>
          <div className="expert-setup__group">
            {expert.optional.map((o) => {
              const st = statuses[o.slug] ?? { kind: 'idle' };
              const name = isZh ? (o.name_zh ?? o.name) : o.name;
              return (
                <div key={o.slug} className="expert-setup__item">
                  <span className="expert-setup__item-name">{name}</span>
                  {o.installed ? (
                    <span className="expert-setup__ok">✓ {es.installed}</span>
                  ) : (
                    <button
                      type="button"
                      className="expert-setup__install"
                      disabled={st.kind === 'installing'}
                      onClick={() => void installMcp(o.slug)}
                    >
                      {st.kind === 'installing' ? es.installing : es.install}
                    </button>
                  )}
                </div>
              );
            })}
          </div>
        </>
      )}

      {offlineHint && (
        <p className="expert-setup__offline">
          <span className="expert-setup__offline-label">{es.offline_label}</span> {offlineHint}
        </p>
      )}
    </div>
  );
}

/** One required-group item: an installable MCP, or a host package with a command. */
function ItemRow({
  item,
  status,
  copied,
  labels,
  onInstall,
  onCopy,
}: {
  item: ExpertItemReadiness;
  status: RowStatus;
  copied: string | null;
  labels: {
    install: string;
    installed: string;
    installing: string;
    install_failed: string;
    host_install: string;
    copy: string;
    copied: string;
    probe_ok: string;
    probe_missing: string;
  };
  onInstall: () => void;
  onCopy: (text: string) => void;
}) {
  const isPackage = item.kind === 'package';
  return (
    <div className="expert-setup__item">
      <div className="expert-setup__item-main">
        <span className="expert-setup__item-name">{item.label}</span>
        {isPackage && item.install && (
          <code className="expert-setup__cmd" title={item.install}>
            {labels.host_install}: {item.install}
          </code>
        )}
        {status.kind === 'failed' && (
          <span className="expert-setup__row-error">
            {interpolate(labels.install_failed, { message: status.message })}
          </span>
        )}
      </div>
      {item.satisfied ? (
        <span className="expert-setup__ok">
          ✓ {isPackage ? labels.probe_ok : labels.installed}
        </span>
      ) : isPackage ? (
        <div className="expert-setup__pkg-actions">
          <span className="expert-setup__missing">✗ {labels.probe_missing}</span>
          {item.install && (
            <button type="button" className="expert-setup__copy" onClick={() => onCopy(item.install!)}>
              {copied === item.install ? labels.copied : labels.copy}
            </button>
          )}
        </div>
      ) : (
        <button
          type="button"
          className="expert-setup__install"
          disabled={status.kind === 'installing'}
          onClick={onInstall}
        >
          {status.kind === 'installing' ? labels.installing : labels.install}
        </button>
      )}
    </div>
  );
}
