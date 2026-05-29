/**
 * v1.8.0 (sprint-10b S10b-3) — Skill Proposals admin pane.
 *
 * Lists agent-authored skill proposals (`GET /v1/skills/proposals`) and
 * lets an admin approve or reject each one. Both mutating actions are
 * gated behind `<RequireScope name="skill.approve">` — the same scope
 * the backend Casbin policy checks. The scope context (sprint-10b S10b-6)
 * fails open when `/v1/admin/me/scopes` is absent, so older deployments
 * keep working.
 *
 * Tenant scoping: like the Personas pane, the backend requires a
 * `tenant_id` query param on the list call. The pane remembers the
 * operator's last choice in `localStorage`.
 *
 * Backend gap (DEC-014): the Rust `approve_proposal_handler` does NOT
 * accept an optional reviewer comment — only `decided_by` is recognised.
 * The original spec asked for `approveSkillProposal(id, comment?)`;
 * documenting the gap in the commit message rather than changing the
 * backend per "don't touch backend" rule.
 */

import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type {
  SkillProposal,
  SkillProposalStatus,
  XiaoguaiClient,
} from '@xiaoguai/shared';
import { ApiError } from '@xiaoguai/shared';
import { client as defaultClient } from '../client';
import { RequireScope } from '../components/RequireScope';
import { SkillManifestPreview } from '../components/SkillManifestPreview';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type StatusFilter = 'pending' | 'all';

type LoadState =
  | { kind: 'loading' }
  | { kind: 'ok'; proposals: SkillProposal[] }
  | { kind: 'unavailable' }
  | { kind: 'error'; message: string };

type ModalState =
  | { kind: 'closed' }
  | { kind: 'approve'; proposal: SkillProposal }
  | { kind: 'reject'; proposal: SkillProposal };

interface Toast {
  id: number;
  message: string;
  kind: 'success' | 'error';
}

// ---------------------------------------------------------------------------
// Pure helpers (testable without DOM)
// ---------------------------------------------------------------------------

/**
 * Filter a proposal list against the selected status filter. The backend
 * already filters when `status` is supplied, but doing it client-side as
 * well keeps the UI responsive when toggling between "pending" and "all"
 * without an extra round trip.
 */
export function filterByStatus(
  proposals: SkillProposal[],
  status: StatusFilter,
): SkillProposal[] {
  if (status === 'all') return proposals;
  return proposals.filter((p) => p.status === 'pending');
}

export function statusToQuery(
  status: StatusFilter,
): SkillProposalStatus | undefined {
  return status === 'pending' ? 'pending' : undefined;
}

export function statusClassName(status: SkillProposalStatus): string {
  // Mirror the kind-tag-* convention used elsewhere.
  if (status === 'pending') return 'kind-tag kind-tag-scheduled';
  if (status === 'approved' || status === 'installed') {
    return 'kind-tag kind-tag-chat';
  }
  return 'kind-tag kind-tag-im';
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

const TENANT_STORAGE_KEY = 'xiaoguai_admin_proposals_tenant';
/**
 * The backend records `decided_by` to audit who approved/rejected. The
 * frontend doesn't yet have a logged-in user identity wired through, so
 * we default to a sentinel that's obvious in audit logs. Operators can
 * override per-action in the comment modal once auth context lands.
 */
const DEFAULT_DECIDED_BY = 'admin-ui';

export interface SkillProposalsPaneProps {
  /** Override the shared client (used by tests). */
  client?: Pick<
    XiaoguaiClient,
    'listSkillProposals' | 'approveSkillProposal' | 'rejectSkillProposal'
  >;
}

export function SkillProposalsPane({
  client,
}: SkillProposalsPaneProps = {}): JSX.Element {
  const c = client ?? defaultClient;
  const { t } = useTranslation();

  const [tenantId, setTenantId] = useState<string>(() => {
    if (typeof localStorage === 'undefined') return '';
    return localStorage.getItem(TENANT_STORAGE_KEY) ?? '';
  });
  const [statusFilter, setStatusFilter] = useState<StatusFilter>('pending');
  const [load, setLoad] = useState<LoadState>({ kind: 'loading' });
  const [modal, setModal] = useState<ModalState>({ kind: 'closed' });
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [rejectReason, setRejectReason] = useState('');
  const [reasonError, setReasonError] = useState<string | null>(null);
  const [toasts, setToasts] = useState<Toast[]>([]);

  useEffect(() => {
    if (typeof localStorage === 'undefined') return;
    if (tenantId) localStorage.setItem(TENANT_STORAGE_KEY, tenantId);
  }, [tenantId]);

  const refresh = useCallback(async () => {
    if (!tenantId.trim()) {
      setLoad({ kind: 'ok', proposals: [] });
      return;
    }
    setLoad({ kind: 'loading' });
    try {
      const proposals = await c.listSkillProposals({
        tenant_id: tenantId.trim(),
        status: statusToQuery(statusFilter),
      });
      setLoad({ kind: 'ok', proposals });
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
  }, [c, tenantId, statusFilter]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const filtered = useMemo(() => {
    if (load.kind !== 'ok') return [];
    return filterByStatus(load.proposals, statusFilter);
  }, [load, statusFilter]);

  function pushToast(message: string, kind: Toast['kind']) {
    const id = Date.now() + Math.random();
    setToasts((prev) => [...prev, { id, message, kind }]);
    setTimeout(
      () => setToasts((prev) => prev.filter((tt) => tt.id !== id)),
      4000,
    );
  }

  function openApprove(p: SkillProposal) {
    setActionError(null);
    setModal({ kind: 'approve', proposal: p });
  }

  function openReject(p: SkillProposal) {
    setActionError(null);
    setRejectReason('');
    setReasonError(null);
    setModal({ kind: 'reject', proposal: p });
  }

  function closeModal() {
    setModal({ kind: 'closed' });
    setActionError(null);
    setRejectReason('');
    setReasonError(null);
  }

  async function onConfirmApprove() {
    if (modal.kind !== 'approve') return;
    const target = modal.proposal;
    setBusy(true);
    setActionError(null);
    try {
      await c.approveSkillProposal(target.id, {
        decided_by: DEFAULT_DECIDED_BY,
      });
      // Optimistic — drop the row from the list immediately.
      if (load.kind === 'ok') {
        setLoad({
          kind: 'ok',
          proposals: load.proposals.filter((p) => p.id !== target.id),
        });
      }
      pushToast(
        t('pane.skill_proposals.toast_approved', { name: target.manifest.name }),
        'success',
      );
      setModal({ kind: 'closed' });
    } catch (err) {
      setActionError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function onConfirmReject() {
    if (modal.kind !== 'reject') return;
    const trimmed = rejectReason.trim();
    if (trimmed === '') {
      setReasonError(t('pane.skill_proposals.reject_reason_required'));
      return;
    }
    const target = modal.proposal;
    setBusy(true);
    setActionError(null);
    try {
      await c.rejectSkillProposal(target.id, {
        decided_by: DEFAULT_DECIDED_BY,
        reason: trimmed,
      });
      if (load.kind === 'ok') {
        setLoad({
          kind: 'ok',
          proposals: load.proposals.filter((p) => p.id !== target.id),
        });
      }
      pushToast(
        t('pane.skill_proposals.toast_rejected', { name: target.manifest.name }),
        'success',
      );
      setModal({ kind: 'closed' });
    } catch (err) {
      setActionError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  // -- render --------------------------------------------------------------

  return (
    <section aria-labelledby="skill-proposals-title" className="pane">
      <header>
        <h1 id="skill-proposals-title">
          {t('pane.skill_proposals.title')}
        </h1>
        <p className="muted">{t('pane.skill_proposals.subtitle')}</p>
      </header>

      <div className="toast-stack" aria-live="polite">
        {toasts.map((tt) => (
          <div key={tt.id} className={`toast toast--${tt.kind}`}>
            {tt.message}
          </div>
        ))}
      </div>

      <div className="toolbar" role="search" aria-label="proposals filters">
        <label>
          <span>{t('pane.skill_proposals.tenant_id_label')}</span>
          <input
            type="text"
            value={tenantId}
            placeholder={t('pane.skill_proposals.tenant_id_placeholder')}
            onChange={(e) => setTenantId(e.target.value)}
            aria-label={t('pane.skill_proposals.tenant_id_label')}
          />
        </label>
        <label>
          <span>{t('pane.skill_proposals.filter_status_label')}</span>
          <select
            value={statusFilter}
            onChange={(e) => setStatusFilter(e.target.value as StatusFilter)}
            aria-label={t('pane.skill_proposals.filter_status_label')}
          >
            <option value="pending">
              {t('pane.skill_proposals.filter_status_pending')}
            </option>
            <option value="all">
              {t('pane.skill_proposals.filter_status_all')}
            </option>
          </select>
        </label>
      </div>

      {load.kind === 'loading' && (
        <p role="status">{t('pane.skill_proposals.loading')}</p>
      )}
      {load.kind === 'unavailable' && (
        <p role="alert" className="alert">
          {t('pane.skill_proposals.unavailable')}
        </p>
      )}
      {load.kind === 'error' && (
        <p role="alert" className="alert">
          {t('common.failed', { message: load.message })}
        </p>
      )}
      {load.kind === 'ok' && filtered.length === 0 && (
        <p role="status" className="muted">
          {t('pane.skill_proposals.empty')}
        </p>
      )}
      {load.kind === 'ok' && filtered.length > 0 && (
        <ul
          className="skill-proposals-list"
          aria-label="skill proposals"
          style={{ listStyle: 'none', padding: 0 }}
        >
          {filtered.map((p) => (
            <li
              key={p.id}
              className="skill-proposal-card"
              data-proposal-id={p.id}
            >
              <div className="skill-proposal-card__status">
                <span className={statusClassName(p.status)}>{p.status}</span>
              </div>
              <SkillManifestPreview
                manifest={p.manifest}
                proposedBy={p.proposed_by}
                submittedAt={p.created_at}
              />
              {p.status === 'pending' && (
                <div
                  className="skill-proposal-card__actions"
                  role="group"
                  aria-label={`actions for ${p.manifest.name}`}
                >
                  <RequireScope name="skill.approve">
                    <button
                      type="button"
                      onClick={() => openApprove(p)}
                      aria-label={`approve ${p.manifest.name}`}
                    >
                      {t('pane.skill_proposals.btn_approve')}
                    </button>
                  </RequireScope>{' '}
                  <RequireScope name="skill.approve">
                    <button
                      type="button"
                      onClick={() => openReject(p)}
                      aria-label={`reject ${p.manifest.name}`}
                    >
                      {t('pane.skill_proposals.btn_reject')}
                    </button>
                  </RequireScope>
                </div>
              )}
              {p.status === 'rejected' && p.reason && (
                <p className="muted skill-proposal-card__reason">
                  <strong>
                    {t('pane.skill_proposals.field_reject_reason')}:
                  </strong>{' '}
                  {p.reason}
                </p>
              )}
            </li>
          ))}
        </ul>
      )}

      {modal.kind === 'approve' && (
        <div className="drawer-backdrop" role="dialog" aria-modal="true">
          <div className="drawer">
            <div className="drawer-header">
              <h2>{t('pane.skill_proposals.approve_title')}</h2>
              <button
                type="button"
                className="drawer-close"
                onClick={closeModal}
                aria-label={t('common.close')}
              >
                ×
              </button>
            </div>
            <p>
              {t('pane.skill_proposals.approve_body', {
                name: modal.proposal.manifest.name,
                version: modal.proposal.manifest.version,
              })}
            </p>
            {actionError && (
              <p role="alert" className="alert">
                {actionError}
              </p>
            )}
            <div className="drawer-actions">
              <button type="button" onClick={closeModal}>
                {t('pane.skill_proposals.btn_cancel')}
              </button>
              <button
                type="button"
                onClick={() => {
                  void onConfirmApprove();
                }}
                disabled={busy}
              >
                {t('pane.skill_proposals.btn_confirm_approve')}
              </button>
            </div>
          </div>
        </div>
      )}

      {modal.kind === 'reject' && (
        <div className="drawer-backdrop" role="dialog" aria-modal="true">
          <div className="drawer">
            <div className="drawer-header">
              <h2>{t('pane.skill_proposals.reject_title')}</h2>
              <button
                type="button"
                className="drawer-close"
                onClick={closeModal}
                aria-label={t('common.close')}
              >
                ×
              </button>
            </div>
            <p>
              {t('pane.skill_proposals.reject_body', {
                name: modal.proposal.manifest.name,
              })}
            </p>
            <form
              onSubmit={(e) => {
                e.preventDefault();
                void onConfirmReject();
              }}
            >
              <label>
                <span>{t('pane.skill_proposals.reject_reason_label')}</span>
                <textarea
                  rows={4}
                  value={rejectReason}
                  placeholder={t(
                    'pane.skill_proposals.reject_reason_placeholder',
                  )}
                  onChange={(e) => {
                    setRejectReason(e.target.value);
                    if (reasonError) setReasonError(null);
                  }}
                  aria-label={t('pane.skill_proposals.reject_reason_label')}
                  aria-invalid={reasonError !== null}
                />
              </label>
              {reasonError && (
                <p role="alert" className="alert">
                  {reasonError}
                </p>
              )}
              {actionError && (
                <p role="alert" className="alert">
                  {actionError}
                </p>
              )}
              <div className="drawer-actions">
                <button type="button" onClick={closeModal}>
                  {t('pane.skill_proposals.btn_cancel')}
                </button>
                <button type="submit" disabled={busy}>
                  {t('pane.skill_proposals.btn_confirm_reject')}
                </button>
              </div>
            </form>
          </div>
        </div>
      )}
    </section>
  );
}
