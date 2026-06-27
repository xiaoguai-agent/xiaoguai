/**
 * v0.12.x.1 — Admin Scheduler pane.
 *
 * Two tabs:
 *
 * 1. **Jobs** — `GET /v1/admin/scheduler/jobs` table with id / name /
 *    trigger / enabled / last_fire_at / next_fire_at / Run-now button.
 *    Includes a small "Tokens" subsection: webhook tokens
 *    (list + create + revoke).
 *
 * 2. **Create from description** — `POST /v1/admin/scheduler/jobs/compile`
 *    → preview JSON → Save → `POST /v1/admin/scheduler/jobs`.
 *
 * Mirrors the `Today` / `Eval` panes' shape: top header, useEffect-based
 * lazy load, error banners surface failures verbatim. Browser eyeball
 * pass deferred to human (same caveat as every admin-ui tag).
 */

import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type {
  CompileScheduledJobResponse,
  ScheduledJobSummary,
  WebhookToken,
} from '@xiaoguai/shared';
import { client } from '../client';
import { ErrorBanner } from '../components/ErrorBanner';

type Tab = 'jobs' | 'create';

export function SchedulerPane() {
  const { t } = useTranslation();
  const [tab, setTab] = useState<Tab>('jobs');
  return (
    <>
      <header className="scheduler-header">
        <h1>{t('pane.scheduler.title')}</h1>
        <p className="muted">{t('pane.scheduler.description')}</p>
      </header>
      <div className="scheduler-tabs" role="tablist">
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'jobs'}
          className={tab === 'jobs' ? 'active' : ''}
          onClick={() => setTab('jobs')}
        >
          {t('pane.scheduler.tab_jobs')}
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'create'}
          className={tab === 'create' ? 'active' : ''}
          onClick={() => setTab('create')}
        >
          {t('pane.scheduler.tab_create')}
        </button>
      </div>
      {tab === 'jobs' ? <JobsTab /> : <CreateTab />}
    </>
  );
}

// ---- Jobs tab -----------------------------------------------------------

function JobsTab(): JSX.Element {
  const { t } = useTranslation();
  const [jobs, setJobs] = useState<ScheduledJobSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [fireBusy, setFireBusy] = useState<string | null>(null);
  const [fireMsg, setFireMsg] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const got = await client.listScheduledJobs({ limit: 200 });
      setJobs(got);
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const fireNow = useCallback(
    async (id: string) => {
      setFireBusy(id);
      setFireMsg(null);
      try {
        await client.fireScheduledJob(id);
        setFireMsg(t('pane.scheduler.fired_ok', { id }));
      } catch (err) {
        setFireMsg(t('common.failed', { message: (err as Error).message }));
      } finally {
        setFireBusy(null);
      }
    },
    [t],
  );

  return (
    <>
      <div className="scheduler-actions">
        <button type="button" onClick={() => void refresh()} disabled={loading}>
          {loading ? t('common.loading') : t('common.refresh')}
        </button>
        {fireMsg && <span className="muted">{fireMsg}</span>}
      </div>
      <ErrorBanner message={error} />
      {jobs === null ? (
        <div className="empty">{t('pane.scheduler.jobs_empty_loading')}</div>
      ) : jobs.length === 0 ? (
        <div className="empty">{t('pane.scheduler.jobs_empty_none')}</div>
      ) : (
        <table className="scheduler-table">
          <thead>
            <tr>
              <th>{t('pane.scheduler.col_id')}</th>
              <th>{t('pane.scheduler.col_name')}</th>
              <th>{t('pane.scheduler.col_trigger')}</th>
              <th>{t('pane.scheduler.col_enabled')}</th>
              <th>{t('pane.scheduler.col_last_fired')}</th>
              <th>{t('pane.scheduler.col_next_fire')}</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {jobs.map((j) => (
              <tr key={j.id}>
                <td>
                  <code>{j.id}</code>
                </td>
                <td>{j.name}</td>
                <td>
                  <code>{j.trigger_summary}</code>
                </td>
                <td>{j.enabled ? t('pane.scheduler.enabled_yes') : t('pane.scheduler.enabled_no')}</td>
                <td>{formatTs(j.last_fire_at)}</td>
                <td>{formatTs(j.next_fire_at)}</td>
                <td>
                  <button
                    type="button"
                    onClick={() => void fireNow(j.id)}
                    disabled={fireBusy === j.id}
                  >
                    {fireBusy === j.id ? t('pane.scheduler.btn_firing') : t('pane.scheduler.btn_run_now')}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <hr className="scheduler-divider" />
      <TokensSection />
    </>
  );
}

function TokensSection(): JSX.Element {
  const { t } = useTranslation();
  const [tokens, setTokens] = useState<WebhookToken[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [createRoute, setCreateRoute] = useState('');
  const [createdToken, setCreatedToken] = useState<WebhookToken | null>(null);

  const refresh = useCallback(async () => {
    setError(null);
    try {
      const got = await client.listWebhookTokens();
      setTokens(got);
    } catch (err) {
      setError((err as Error).message);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const create = useCallback(async () => {
    if (!createRoute.trim()) return;
    try {
      const row = await client.createWebhookToken({
        route_id: createRoute.trim(),
      });
      setCreatedToken(row);
      setCreateRoute('');
      await refresh();
    } catch (err) {
      setError((err as Error).message);
    }
  }, [createRoute, refresh]);

  const revoke = useCallback(
    async (tok: string) => {
      try {
        await client.revokeWebhookToken(tok);
        await refresh();
      } catch (err) {
        setError((err as Error).message);
      }
    },
    [refresh],
  );

  return (
    <section className="tokens-section">
      <h2>{t('pane.scheduler.tokens_title')}</h2>
      <p className="muted">{t('pane.scheduler.tokens_description')}</p>
      <div className="tokens-create">
        <input
          type="text"
          placeholder={t('pane.scheduler.tokens_placeholder_route')}
          value={createRoute}
          onChange={(e) => setCreateRoute(e.target.value)}
        />
        <button
          type="button"
          onClick={() => void create()}
          disabled={!createRoute.trim()}
        >
          {t('pane.scheduler.tokens_btn_mint')}
        </button>
      </div>
      {createdToken && (
        <div className="token-mint">
          <strong>{t('pane.scheduler.tokens_new_token')}</strong>{' '}
          <code>{createdToken.token}</code>{' '}
          <button type="button" onClick={() => setCreatedToken(null)}>
            {t('pane.scheduler.tokens_btn_dismiss')}
          </button>
        </div>
      )}
      <ErrorBanner message={error} />
      {tokens === null ? (
        <div className="empty">{t('pane.scheduler.tokens_empty_loading')}</div>
      ) : tokens.length === 0 ? (
        <div className="empty">{t('pane.scheduler.tokens_empty_none')}</div>
      ) : (
        <table className="tokens-table">
          <thead>
            <tr>
              <th>{t('pane.scheduler.tokens_col_token')}</th>
              <th>{t('pane.scheduler.tokens_col_route')}</th>
              <th>{t('pane.scheduler.tokens_col_created')}</th>
              <th>{t('pane.scheduler.tokens_col_last_used')}</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {tokens.map((tok) => (
              <tr key={tok.token}>
                <td>
                  <code>{shortToken(tok.token)}</code>
                </td>
                <td>
                  <code>{tok.route_id}</code>
                </td>
                <td>{formatTs(tok.created_at)}</td>
                <td>{formatTs(tok.last_used_at ?? null)}</td>
                <td>
                  <button type="button" onClick={() => void revoke(tok.token)}>
                    {t('pane.scheduler.tokens_btn_revoke')}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

// ---- Create tab ---------------------------------------------------------

function CreateTab(): JSX.Element {
  const { t } = useTranslation();
  const [description, setDescription] = useState('');
  const [preview, setPreview] = useState<CompileScheduledJobResponse | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<'compile' | 'save' | null>(null);
  const [saveMsg, setSaveMsg] = useState<string | null>(null);

  const compile = useCallback(async () => {
    if (!description.trim()) return;
    setBusy('compile');
    setError(null);
    setPreview(null);
    setSaveMsg(null);
    try {
      const resp = await client.compileScheduledJob({
        description: description.trim(),
      });
      setPreview(resp);
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(null);
    }
  }, [description]);

  const save = useCallback(async () => {
    if (preview === null) return;
    setBusy('save');
    setError(null);
    setSaveMsg(null);
    try {
      const r = await client.upsertScheduledJob(preview.suggested_job);
      setSaveMsg(t('pane.scheduler.create_saved', { id: r.id }));
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(null);
    }
  }, [preview, t]);

  return (
    <div className="scheduler-create">
      <label>
        {t('pane.scheduler.create_label_description')}
        <textarea
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          rows={4}
          placeholder={t('pane.scheduler.create_placeholder_description')}
        />
      </label>
      <div className="scheduler-create-actions">
        <button
          type="button"
          onClick={() => void compile()}
          disabled={busy !== null || !description.trim()}
        >
          {busy === 'compile' ? t('pane.scheduler.create_btn_compiling') : t('pane.scheduler.create_btn_compile')}
        </button>
        <button
          type="button"
          onClick={() => void save()}
          disabled={busy !== null || preview === null}
        >
          {busy === 'save' ? t('pane.scheduler.create_btn_saving') : t('pane.scheduler.create_btn_save')}
        </button>
        {saveMsg && <span className="muted">{saveMsg}</span>}
      </div>
      <ErrorBanner message={error} />
      {preview && (
        <div className="scheduler-preview">
          <p className="muted">{preview.rationale}</p>
          <pre>
            <code>{JSON.stringify(preview.suggested_job, null, 2)}</code>
          </pre>
        </div>
      )}
    </div>
  );
}

// ---- helpers ------------------------------------------------------------

function formatTs(iso: string | null): string {
  if (!iso) return '—';
  try {
    return new Date(iso).toLocaleString();
  } catch {
    return iso;
  }
}

function shortToken(tok: string): string {
  if (tok.length <= 12) return tok;
  return `${tok.slice(0, 6)}…${tok.slice(-4)}`;
}
