/**
 * T6 self-healing (DEC-040) — Incidents pane.
 *
 * Owner-facing surface over the incident backend (#277): a status-filtered
 * list, a detail drawer (incident + RCA + repair history), the four operator
 * actions — Analyze, Approve repair, Dismiss, View report — and a manual
 * "New incident" form. Manual create uses the owner-authed `POST /v1/incidents`
 * (NOT the token-gated ingest webhook).
 *
 * Mutating actions are gated behind `<RequireScope name="incidents.write">`
 * (create / analyze / dismiss) and `"incidents.approve"` (approve-repair);
 * single-owner fail-open per hooks/useScopes.tsx. Renders Loading / Error /
 * Empty / Unavailable (503) / Ready states (LLD-ADMIN-UI-001 §4.1).
 */

import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type {
  CreateIncidentRequest,
  IncidentDetails,
  IncidentRecord,
  IncidentSeverity,
  IncidentStatus,
  RcaRecord,
  XiaoguaiClient,
} from '@xiaoguai/shared';
import { ApiError } from '@xiaoguai/shared';
import { client as defaultClient } from '../client';
import { RequireScope } from '../components/RequireScope';
import { PaneIntro } from '../components/PaneIntro';
import { fmtDate } from './Personas';

// ---------------------------------------------------------------------------
// Pure helpers (testable without DOM)
// ---------------------------------------------------------------------------

export const INCIDENT_STATUSES: IncidentStatus[] = [
  'open',
  'analyzing',
  'awaiting_approval',
  'repairing',
  'resolved',
  'failed',
  'dismissed',
];

export const INCIDENT_SEVERITIES: IncidentSeverity[] = [
  'critical',
  'high',
  'medium',
  'low',
];

export interface IncidentFormState {
  title: string;
  severity: IncidentSeverity;
  project: string;
  environment: string;
}

export const EMPTY_INCIDENT_FORM: IncidentFormState = {
  title: '',
  severity: 'medium',
  project: '',
  environment: '',
};

/** Build the owner-authed create body; blank optional fields are omitted. */
export function formToCreateIncidentReq(
  f: IncidentFormState,
): CreateIncidentRequest {
  const req: CreateIncidentRequest = {
    title: f.title.trim(),
    severity: f.severity,
  };
  const project = f.project.trim();
  const environment = f.environment.trim();
  if (project) req.project = project;
  if (environment) req.environment = environment;
  return req;
}

export type IncidentFormProblem = 'no_title' | null;

export function validateIncidentForm(f: IncidentFormState): IncidentFormProblem {
  return f.title.trim() === '' ? 'no_title' : null;
}

export function isTerminalStatus(s: IncidentStatus): boolean {
  return s === 'resolved' || s === 'failed' || s === 'dismissed';
}

/** Status machine gates mirrored from the backend (`can_transition_to`). */
export const canAnalyze = (s: IncidentStatus): boolean => s === 'open';
export const canApprove = (s: IncidentStatus): boolean =>
  s === 'awaiting_approval';
export const canDismiss = (s: IncidentStatus): boolean => !isTerminalStatus(s);

/** Newest RCA (the backend returns rcas newest-first); null when none. */
export function latestRca(details: IncidentDetails): RcaRecord | null {
  return details.rcas.length > 0 ? details.rcas[0]! : null;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

type Load =
  | { kind: 'loading' }
  | { kind: 'ok'; incidents: IncidentRecord[] }
  | { kind: 'unavailable' }
  | { kind: 'error'; message: string };

type Detail =
  | { kind: 'closed' }
  | { kind: 'loading' }
  | { kind: 'ok'; details: IncidentDetails }
  | { kind: 'error'; message: string };

type Confirm =
  | { kind: 'idle' }
  | { kind: 'approve'; rcaId: string }
  | { kind: 'dismiss' };

export interface IncidentsPaneProps {
  /** Override the shared client (used by tests). */
  client?: Pick<
    XiaoguaiClient,
    | 'listIncidents'
    | 'getIncident'
    | 'createIncident'
    | 'analyzeIncident'
    | 'approveRepair'
    | 'dismissIncident'
    | 'incidentReport'
  >;
}

export function IncidentsPane({
  client,
}: IncidentsPaneProps = {}): JSX.Element {
  const c = client ?? defaultClient;
  const { t } = useTranslation();

  const [statusFilter, setStatusFilter] = useState<IncidentStatus | 'all'>(
    'all',
  );
  const [load, setLoad] = useState<Load>({ kind: 'loading' });

  const [createOpen, setCreateOpen] = useState(false);
  const [form, setForm] = useState<IncidentFormState>(EMPTY_INCIDENT_FORM);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const [detailId, setDetailId] = useState<string | null>(null);
  const [detail, setDetail] = useState<Detail>({ kind: 'closed' });
  const [report, setReport] = useState<string | null>(null);
  const [acting, setActing] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<Confirm>({ kind: 'idle' });

  const refresh = useCallback(async () => {
    setLoad({ kind: 'loading' });
    try {
      const incidents = await c.listIncidents(
        statusFilter === 'all' ? undefined : statusFilter,
      );
      setLoad({ kind: 'ok', incidents });
    } catch (err) {
      if (err instanceof ApiError && err.status === 503) {
        setLoad({ kind: 'unavailable' });
        return;
      }
      setLoad({
        kind: 'error',
        message: err instanceof Error ? err.message : String(err),
      });
    }
  }, [c, statusFilter]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const loadDetail = useCallback(
    async (id: string) => {
      setDetail({ kind: 'loading' });
      setReport(null);
      setActionError(null);
      try {
        const details = await c.getIncident(id);
        setDetail({ kind: 'ok', details });
      } catch (err) {
        setDetail({
          kind: 'error',
          message: err instanceof Error ? err.message : String(err),
        });
      }
    },
    [c],
  );

  function openDetail(id: string) {
    setDetailId(id);
    setConfirm({ kind: 'idle' });
    void loadDetail(id);
  }

  function closeDetail() {
    setDetailId(null);
    setDetail({ kind: 'closed' });
    setReport(null);
    setConfirm({ kind: 'idle' });
    setActionError(null);
  }

  // Run a detail action (analyze / approve / dismiss), then re-fetch the
  // detail and the list (status may have changed).
  async function runAction(fn: () => Promise<unknown>) {
    if (!detailId) return;
    setActing(true);
    setActionError(null);
    try {
      await fn();
      setConfirm({ kind: 'idle' });
      await loadDetail(detailId);
      await refresh();
    } catch (err) {
      setActionError(err instanceof Error ? err.message : String(err));
    } finally {
      setActing(false);
    }
  }

  async function onCreate() {
    if (validateIncidentForm(form) !== null) return;
    setSaving(true);
    setSaveError(null);
    try {
      await c.createIncident(formToCreateIncidentReq(form));
      setCreateOpen(false);
      setForm(EMPTY_INCIDENT_FORM);
      await refresh();
    } catch (err) {
      setSaveError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  }

  async function showReport() {
    if (!detailId) return;
    setActionError(null);
    try {
      setReport(await c.incidentReport(detailId));
    } catch (err) {
      setActionError(err instanceof Error ? err.message : String(err));
    }
  }

  const createProblem = validateIncidentForm(form);
  const incident = detail.kind === 'ok' ? detail.details.incident : null;
  const rca = detail.kind === 'ok' ? latestRca(detail.details) : null;

  // -- render --------------------------------------------------------------

  return (
    <section aria-labelledby="incidents-title" className="pane">
      <header>
        <h1 id="incidents-title">{t('pane.incidents.title')}</h1>
      </header>

      <PaneIntro
        purpose={t('pane.incidents.intro.purpose')}
        usage={t('pane.incidents.intro.usage')}
        usageLabel={t('pane.incidents.intro.usage_label')}
        examplesLabel={t('pane.incidents.intro.examples_label')}
        examples={[t('pane.incidents.intro.example_1')]}
      />

      <div className="toolbar" role="search" aria-label="incidents filters">
        <label>
          <span>{t('pane.incidents.filter_status_label')}</span>{' '}
          <select
            value={statusFilter}
            onChange={(e) =>
              setStatusFilter(e.target.value as IncidentStatus | 'all')
            }
            aria-label={t('pane.incidents.filter_status_label')}
          >
            <option value="all">{t('pane.incidents.filter_status_all')}</option>
            {INCIDENT_STATUSES.map((s) => (
              <option key={s} value={s}>
                {t(`pane.incidents.status_${s}`)}
              </option>
            ))}
          </select>
        </label>
        <RequireScope name="incidents.write">
          <button
            type="button"
            onClick={() => {
              setForm(EMPTY_INCIDENT_FORM);
              setSaveError(null);
              setCreateOpen(true);
            }}
          >
            {t('pane.incidents.btn_new')}
          </button>
        </RequireScope>
      </div>

      {load.kind === 'loading' && (
        <p role="status">{t('pane.incidents.loading')}</p>
      )}
      {load.kind === 'unavailable' && (
        <p role="alert" className="alert">
          {t('pane.incidents.unavailable')}
        </p>
      )}
      {load.kind === 'error' && (
        <p role="alert" className="alert">
          {t('common.failed', { message: load.message })}
        </p>
      )}
      {load.kind === 'ok' && load.incidents.length === 0 && (
        <p role="status" className="muted">
          {t('pane.incidents.empty')}
        </p>
      )}
      {load.kind === 'ok' && load.incidents.length > 0 && (
        <table aria-label="incidents">
          <thead>
            <tr>
              <th>{t('pane.incidents.col_title')}</th>
              <th>{t('pane.incidents.col_source')}</th>
              <th>{t('pane.incidents.col_severity')}</th>
              <th>{t('pane.incidents.col_status')}</th>
              <th>{t('pane.incidents.col_created')}</th>
              <th>{t('pane.incidents.col_actions')}</th>
            </tr>
          </thead>
          <tbody>
            {load.incidents.map((inc) => (
              <tr key={inc.id}>
                <td>{inc.title}</td>
                <td>{inc.source}</td>
                <td>
                  <span className="kind-tag">
                    {t(`pane.incidents.sev_${inc.severity}`)}
                  </span>
                </td>
                <td>
                  <span className="kind-tag">
                    {t(`pane.incidents.status_${inc.status}`)}
                  </span>
                </td>
                <td>{fmtDate(inc.created_at)}</td>
                <td>
                  <button
                    type="button"
                    onClick={() => openDetail(inc.id)}
                    aria-label={`view ${inc.title}`}
                  >
                    {t('pane.incidents.btn_view')}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {/* Create drawer */}
      {createOpen && (
        <div className="drawer-backdrop" role="dialog" aria-modal="true">
          <div className="drawer">
            <div className="drawer-header">
              <h2>{t('pane.incidents.drawer_create_title')}</h2>
              <button
                type="button"
                className="drawer-close"
                onClick={() => setCreateOpen(false)}
                aria-label={t('common.close')}
              >
                ×
              </button>
            </div>
            <form
              onSubmit={(e) => {
                e.preventDefault();
                void onCreate();
              }}
            >
              <label>
                <span>{t('pane.incidents.field_title')}</span>
                <input
                  type="text"
                  value={form.title}
                  placeholder={t('pane.incidents.placeholder_title')}
                  onChange={(e) => setForm({ ...form, title: e.target.value })}
                  required
                />
              </label>
              <label>
                <span>{t('pane.incidents.field_severity')}</span>
                <select
                  value={form.severity}
                  onChange={(e) =>
                    setForm({
                      ...form,
                      severity: e.target.value as IncidentSeverity,
                    })
                  }
                  aria-label={t('pane.incidents.field_severity')}
                >
                  {INCIDENT_SEVERITIES.map((s) => (
                    <option key={s} value={s}>
                      {t(`pane.incidents.sev_${s}`)}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                <span>{t('pane.incidents.field_project')}</span>
                <input
                  type="text"
                  value={form.project}
                  placeholder={t('pane.incidents.placeholder_project')}
                  onChange={(e) =>
                    setForm({ ...form, project: e.target.value })
                  }
                />
              </label>
              <label>
                <span>{t('pane.incidents.field_environment')}</span>
                <input
                  type="text"
                  value={form.environment}
                  placeholder={t('pane.incidents.placeholder_environment')}
                  onChange={(e) =>
                    setForm({ ...form, environment: e.target.value })
                  }
                />
              </label>
              {createProblem === 'no_title' && (
                <p role="status" className="muted">
                  {t('pane.incidents.hint_no_title')}
                </p>
              )}
              {saveError && (
                <p role="alert" className="alert">
                  {saveError}
                </p>
              )}
              <div className="drawer-actions">
                <button type="button" onClick={() => setCreateOpen(false)}>
                  {t('pane.incidents.btn_cancel')}
                </button>
                <button type="submit" disabled={saving || createProblem !== null}>
                  {t('pane.incidents.btn_create')}
                </button>
              </div>
            </form>
          </div>
        </div>
      )}

      {/* Detail drawer */}
      {detail.kind !== 'closed' && (
        <div className="drawer-backdrop" role="dialog" aria-modal="true">
          <div className="drawer">
            <div className="drawer-header">
              <h2>{t('pane.incidents.detail_title')}</h2>
              <button
                type="button"
                className="drawer-close"
                onClick={closeDetail}
                aria-label={t('common.close')}
              >
                ×
              </button>
            </div>

            {detail.kind === 'loading' && (
              <p role="status">{t('pane.incidents.detail_loading')}</p>
            )}
            {detail.kind === 'error' && (
              <p role="alert" className="alert">
                {t('common.failed', { message: detail.message })}
              </p>
            )}

            {detail.kind === 'ok' && incident && (
              <>
                <h3>{incident.title}</h3>
                <p className="muted">
                  {incident.source} · {incident.external_id}
                </p>
                <dl>
                  <dt>{t('pane.incidents.col_severity')}</dt>
                  <dd>{t(`pane.incidents.sev_${incident.severity}`)}</dd>
                  <dt>{t('pane.incidents.col_status')}</dt>
                  <dd>{t(`pane.incidents.status_${incident.status}`)}</dd>
                  <dt>{t('pane.incidents.field_project')}</dt>
                  <dd>{incident.project}</dd>
                  <dt>{t('pane.incidents.field_environment')}</dt>
                  <dd>{incident.environment ?? '—'}</dd>
                  <dt>{t('pane.incidents.col_created')}</dt>
                  <dd>{fmtDate(incident.created_at)}</dd>
                </dl>

                <div className="drawer-actions">
                  {canAnalyze(incident.status) && (
                    <RequireScope name="incidents.write">
                      <button
                        type="button"
                        disabled={acting}
                        onClick={() =>
                          void runAction(() => c.analyzeIncident(incident.id))
                        }
                      >
                        {t('pane.incidents.btn_analyze')}
                      </button>
                    </RequireScope>
                  )}
                  {canApprove(incident.status) && rca && (
                    <RequireScope name="incidents.approve">
                      <button
                        type="button"
                        disabled={acting}
                        onClick={() =>
                          setConfirm({ kind: 'approve', rcaId: rca.id })
                        }
                      >
                        {t('pane.incidents.btn_approve')}
                      </button>
                    </RequireScope>
                  )}
                  {canDismiss(incident.status) && (
                    <RequireScope name="incidents.write">
                      <button
                        type="button"
                        disabled={acting}
                        onClick={() => setConfirm({ kind: 'dismiss' })}
                      >
                        {t('pane.incidents.btn_dismiss')}
                      </button>
                    </RequireScope>
                  )}
                  <button type="button" disabled={acting} onClick={() => void showReport()}>
                    {t('pane.incidents.btn_report')}
                  </button>
                </div>

                {actionError && (
                  <p role="alert" className="alert">
                    {actionError}
                  </p>
                )}

                <h4>{t('pane.incidents.section_rcas')}</h4>
                {detail.details.rcas.length === 0 ? (
                  <p className="muted">{t('pane.incidents.rca_none')}</p>
                ) : (
                  <ul>
                    {detail.details.rcas.map((r) => (
                      <li key={r.id}>
                        <strong>{r.summary}</strong>
                        <div className="muted">
                          {t('pane.incidents.rca_root_cause')}: {r.root_cause}
                        </div>
                        <div className="muted">
                          {t('pane.incidents.rca_confidence')}:{' '}
                          {r.confidence.toFixed(2)}
                        </div>
                      </li>
                    ))}
                  </ul>
                )}

                <h4>{t('pane.incidents.section_repairs')}</h4>
                {detail.details.repairs.length === 0 ? (
                  <p className="muted">{t('pane.incidents.repairs_none')}</p>
                ) : (
                  <ul>
                    {detail.details.repairs.map((rep) => (
                      <li key={rep.id}>
                        <span className="kind-tag">
                          {rep.ok
                            ? t('pane.incidents.repair_ok')
                            : t('pane.incidents.repair_failed')}
                        </span>{' '}
                        {rep.summary}
                      </li>
                    ))}
                  </ul>
                )}

                {report !== null && (
                  <>
                    <h4>{t('pane.incidents.report_title')}</h4>
                    <pre className="report">{report}</pre>
                  </>
                )}
              </>
            )}
          </div>
        </div>
      )}

      {/* Confirm modal (approve / dismiss) — overlays the detail drawer. */}
      {confirm.kind !== 'idle' && incident && (
        <div className="drawer-backdrop" role="dialog" aria-modal="true">
          <div className="drawer">
            <div className="drawer-header">
              <h2>
                {confirm.kind === 'approve'
                  ? t('pane.incidents.approve_confirm_title')
                  : t('pane.incidents.dismiss_confirm_title')}
              </h2>
            </div>
            <p>
              {confirm.kind === 'approve'
                ? t('pane.incidents.approve_confirm_body')
                : t('pane.incidents.dismiss_confirm_body')}
            </p>
            <p>
              <strong>{incident.title}</strong>
            </p>
            <div className="drawer-actions">
              <button
                type="button"
                onClick={() => setConfirm({ kind: 'idle' })}
              >
                {t('pane.incidents.btn_cancel')}
              </button>
              {confirm.kind === 'approve' ? (
                <button
                  type="button"
                  disabled={acting}
                  onClick={() =>
                    void runAction(() =>
                      c.approveRepair(incident.id, confirm.rcaId),
                    )
                  }
                >
                  {t('pane.incidents.btn_approve')}
                </button>
              ) : (
                <button
                  type="button"
                  disabled={acting}
                  onClick={() =>
                    void runAction(() => c.dismissIncident(incident.id))
                  }
                >
                  {t('pane.incidents.btn_dismiss')}
                </button>
              )}
            </div>
          </div>
        </div>
      )}
    </section>
  );
}
