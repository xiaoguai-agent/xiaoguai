/**
 * v0.12.x.1 — Admin Scheduler pane.
 *
 * Two tabs:
 *
 * 1. **Jobs** — `GET /v1/admin/scheduler/jobs` table with id / name /
 *    trigger / enabled / last_fire_at / next_fire_at / Run-now button.
 *    Includes a small "Tokens" subsection: per-tenant webhook tokens
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
import type {
  CompileScheduledJobResponse,
  ScheduledJobSummary,
  WebhookToken,
} from '@xiaoguai/shared';
import { client } from '../client';

type Tab = 'jobs' | 'create';

export function SchedulerPane() {
  const [tab, setTab] = useState<Tab>('jobs');
  return (
    <>
      <header className="scheduler-header">
        <h1>Scheduler</h1>
        <p className="muted">
          Cron / interval / webhook / file-watch / proactive jobs.
          The runner picks up every change on the next tick (default
          30s).
        </p>
      </header>
      <div className="scheduler-tabs" role="tablist">
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'jobs'}
          className={tab === 'jobs' ? 'active' : ''}
          onClick={() => setTab('jobs')}
        >
          Jobs
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'create'}
          className={tab === 'create' ? 'active' : ''}
          onClick={() => setTab('create')}
        >
          Create from description
        </button>
      </div>
      {tab === 'jobs' ? <JobsTab /> : <CreateTab />}
    </>
  );
}

// ---- Jobs tab -----------------------------------------------------------

function JobsTab(): JSX.Element {
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
        setFireMsg(`Fired ${id}. It will run shortly.`);
      } catch (err) {
        setFireMsg(`Failed: ${(err as Error).message}`);
      } finally {
        setFireBusy(null);
      }
    },
    [],
  );

  return (
    <>
      <div className="scheduler-actions">
        <button type="button" onClick={() => void refresh()} disabled={loading}>
          {loading ? 'Loading…' : 'Refresh'}
        </button>
        {fireMsg && <span className="muted">{fireMsg}</span>}
      </div>
      {error && <div className="error">Failed: {error}</div>}
      {jobs === null ? (
        <div className="empty">Loading…</div>
      ) : jobs.length === 0 ? (
        <div className="empty">No scheduled jobs yet.</div>
      ) : (
        <table className="scheduler-table">
          <thead>
            <tr>
              <th>ID</th>
              <th>Name</th>
              <th>Trigger</th>
              <th>Enabled</th>
              <th>Last fired</th>
              <th>Next fire</th>
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
                <td>{j.enabled ? 'yes' : 'no'}</td>
                <td>{formatTs(j.last_fire_at)}</td>
                <td>{formatTs(j.next_fire_at)}</td>
                <td>
                  <button
                    type="button"
                    onClick={() => void fireNow(j.id)}
                    disabled={fireBusy === j.id}
                  >
                    {fireBusy === j.id ? 'Firing…' : 'Run now'}
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
  const [tokens, setTokens] = useState<WebhookToken[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tenantFilter, setTenantFilter] = useState('');
  const [createTenant, setCreateTenant] = useState('');
  const [createRoute, setCreateRoute] = useState('');
  const [createdToken, setCreatedToken] = useState<WebhookToken | null>(null);

  const refresh = useCallback(async () => {
    setError(null);
    try {
      const opts = tenantFilter.trim()
        ? { tenant_id: tenantFilter.trim() }
        : undefined;
      const got = await client.listWebhookTokens(opts);
      setTokens(got);
    } catch (err) {
      setError((err as Error).message);
    }
  }, [tenantFilter]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const create = useCallback(async () => {
    if (!createTenant.trim() || !createRoute.trim()) return;
    try {
      const row = await client.createWebhookToken({
        tenant_id: createTenant.trim(),
        route_id: createRoute.trim(),
      });
      setCreatedToken(row);
      setCreateTenant('');
      setCreateRoute('');
      await refresh();
    } catch (err) {
      setError((err as Error).message);
    }
  }, [createTenant, createRoute, refresh]);

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
      <h2>Webhook tokens</h2>
      <p className="muted">
        Per-tenant tokens for the public webhook endpoint at{' '}
        <code>POST /v1/scheduler/webhooks/:route_id</code>. Pass via
        <code> X-Xiaoguai-Token</code> header.
      </p>
      <div className="tokens-create">
        <input
          type="text"
          placeholder="tenant_id"
          value={createTenant}
          onChange={(e) => setCreateTenant(e.target.value)}
        />
        <input
          type="text"
          placeholder="route_id"
          value={createRoute}
          onChange={(e) => setCreateRoute(e.target.value)}
        />
        <button
          type="button"
          onClick={() => void create()}
          disabled={!createTenant.trim() || !createRoute.trim()}
        >
          Mint token
        </button>
        <input
          type="text"
          placeholder="filter by tenant_id"
          value={tenantFilter}
          onChange={(e) => setTenantFilter(e.target.value)}
        />
      </div>
      {createdToken && (
        <div className="token-mint">
          <strong>New token (capture now — won't be shown again):</strong>{' '}
          <code>{createdToken.token}</code>{' '}
          <button type="button" onClick={() => setCreatedToken(null)}>
            Dismiss
          </button>
        </div>
      )}
      {error && <div className="error">Failed: {error}</div>}
      {tokens === null ? (
        <div className="empty">Loading…</div>
      ) : tokens.length === 0 ? (
        <div className="empty">No tokens yet.</div>
      ) : (
        <table className="tokens-table">
          <thead>
            <tr>
              <th>Token</th>
              <th>Tenant</th>
              <th>Route</th>
              <th>Created</th>
              <th>Last used</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {tokens.map((t) => (
              <tr key={t.token}>
                <td>
                  <code>{shortToken(t.token)}</code>
                </td>
                <td>{t.tenant_id}</td>
                <td>
                  <code>{t.route_id}</code>
                </td>
                <td>{formatTs(t.created_at)}</td>
                <td>{formatTs(t.last_used_at ?? null)}</td>
                <td>
                  <button type="button" onClick={() => void revoke(t.token)}>
                    Revoke
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
  const [description, setDescription] = useState('');
  const [tenant, setTenant] = useState('');
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
        tenant_id: tenant.trim() ? tenant.trim() : undefined,
      });
      setPreview(resp);
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(null);
    }
  }, [description, tenant]);

  const save = useCallback(async () => {
    if (preview === null) return;
    setBusy('save');
    setError(null);
    setSaveMsg(null);
    try {
      const r = await client.upsertScheduledJob(preview.suggested_job);
      setSaveMsg(`Saved as ${r.id}.`);
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(null);
    }
  }, [preview]);

  return (
    <div className="scheduler-create">
      <label>
        Description
        <textarea
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          rows={4}
          placeholder='e.g. 每天 8 点扫 r/LocalLLaMA + HN，结果推 Telegram'
        />
      </label>
      <label>
        Tenant (optional)
        <input
          type="text"
          value={tenant}
          onChange={(e) => setTenant(e.target.value)}
          placeholder="tenant_id"
        />
      </label>
      <div className="scheduler-create-actions">
        <button
          type="button"
          onClick={() => void compile()}
          disabled={busy !== null || !description.trim()}
        >
          {busy === 'compile' ? 'Compiling…' : 'Compile'}
        </button>
        <button
          type="button"
          onClick={() => void save()}
          disabled={busy !== null || preview === null}
        >
          {busy === 'save' ? 'Saving…' : 'Save'}
        </button>
        {saveMsg && <span className="muted">{saveMsg}</span>}
      </div>
      {error && <div className="error">Failed: {error}</div>}
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

function shortToken(t: string): string {
  if (t.length <= 12) return t;
  return `${t.slice(0, 6)}…${t.slice(-4)}`;
}
