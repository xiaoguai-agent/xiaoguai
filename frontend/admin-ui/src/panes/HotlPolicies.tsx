/**
 * v1.3.x — HotL Policies pane.
 *
 * Full CRUD for `/v1/hotl/policies` with a "Test policy" drawer.
 *
 * Layout:
 *   - 503 graceful banner when PgHotlPolicyStore is not yet wired
 *   - "+ New policy" button → create modal
 *   - List table: scope / max_count / max_usd / window_seconds / escalate_to / Edit / Delete
 *   - Edit: PUT /v1/hotl/policies/{id}
 *   - Delete: confirmation dialog → DELETE /v1/hotl/policies/{id}
 *   - "Test policy" button: drawer with check form → POST /v1/hotl/check
 *   - Empty state when no policies exist
 *
 * 503 fallback: shown when the API responds with HTTP 503. Displays an
 * informative message + link to the Bridges status section.
 *
 * Business rule mirrored from `crates/xiaoguai-api/src/hotl/policy.rs`:
 *   at least one of max_count / max_usd must be set.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type {
  HotlCheckRequest,
  HotlPolicy,
  HotlPolicyCreateRequest,
  HotlVerdict,
} from '@xiaoguai/shared';
import { ApiError } from '@xiaoguai/shared';
import { PaneIntro } from '../components/PaneIntro';
import { client } from '../client';
import { TrustTiers } from '../components/TrustTiers';
import { fmtWindow } from '../utils/window';

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const KNOWN_SCOPES = ['llm_call', 'email_send', 'webhook_invoke'];

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface FormState {
  scope: string;
  window_seconds: string;
  max_count: string;
  max_usd: string;
  escalate_to: string;
}

interface FormErrors {
  scope?: string;
  window_seconds?: string;
  limits?: string;
  max_count?: string;
  max_usd?: string;
}

type ModalState =
  | { kind: 'closed' }
  | { kind: 'create' }
  | { kind: 'edit'; policy: HotlPolicy };

type DeleteState =
  | { kind: 'idle' }
  | { kind: 'confirming'; id: string; scope: string }
  | { kind: 'deleting' };

type CheckState =
  | { kind: 'idle' }
  | { kind: 'running' }
  | { kind: 'result'; verdict: HotlVerdict }
  | { kind: 'error'; message: string };

type LoadState =
  | { kind: 'loading' }
  | { kind: 'ok'; policies: HotlPolicy[] }
  | { kind: 'unavailable' }
  | { kind: 'error'; message: string };

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function buildFormErrors(f: FormState): FormErrors {
  const errs: FormErrors = {};
  if (!f.scope.trim()) errs.scope = 'Scope is required';
  const w = Number(f.window_seconds);
  if (!f.window_seconds.trim() || isNaN(w) || w <= 0) {
    errs.window_seconds = 'Window seconds must be a positive integer';
  }
  const hasCount = f.max_count.trim() !== '';
  const hasUsd = f.max_usd.trim() !== '';
  if (!hasCount && !hasUsd) {
    errs.limits = 'At least one of Max count or Max USD must be set';
  }
  if (hasCount) {
    const c = Number(f.max_count);
    if (isNaN(c) || c <= 0 || !Number.isInteger(c)) {
      errs.max_count = 'Max count must be a positive integer';
    }
  }
  if (hasUsd) {
    const u = Number(f.max_usd);
    if (isNaN(u) || u < 0) {
      errs.max_usd = 'Max USD must be >= 0';
    }
  }
  return errs;
}

function formToRequest(f: FormState): HotlPolicyCreateRequest {
  return {
    scope: f.scope.trim(),
    window_seconds: Number(f.window_seconds),
    max_count: f.max_count.trim() !== '' ? Number(f.max_count) : null,
    max_usd: f.max_usd.trim() !== '' ? Number(f.max_usd) : null,
    escalate_to: f.escalate_to.trim() !== '' ? f.escalate_to.trim() : null,
  };
}

function policyToForm(p: HotlPolicy): FormState {
  return {
    scope: p.scope,
    window_seconds: String(p.window_seconds),
    max_count: p.max_count !== null ? String(p.max_count) : '',
    max_usd: p.max_usd !== null ? String(p.max_usd) : '',
    escalate_to: p.escalate_to ?? '',
  };
}

const EMPTY_FORM: FormState = {
  scope: '',
  window_seconds: '',
  max_count: '',
  max_usd: '',
  escalate_to: '',
};


function is503(err: unknown): boolean {
  return err instanceof ApiError && err.status === 503;
}

// ---------------------------------------------------------------------------
// Policy form (create / edit modal)
// ---------------------------------------------------------------------------

interface PolicyFormProps {
  initial: FormState;
  submitLabel: string;
  onSubmit: (f: FormState) => void;
  onCancel: () => void;
  saving: boolean;
  serverError: string | null;
}

function PolicyForm({
  initial,
  submitLabel,
  onSubmit,
  onCancel,
  saving,
  serverError,
}: PolicyFormProps): JSX.Element {
  const [form, setForm] = useState<FormState>(initial);
  const [touched, setTouched] = useState(false);

  const errors = buildFormErrors(form);
  const hasErrors = Object.keys(errors).length > 0;

  function set(field: keyof FormState, value: string) {
    setForm((prev) => ({ ...prev, [field]: value }));
  }

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setTouched(true);
    if (hasErrors) return;
    onSubmit(form);
  }

  const showErr = (key: keyof FormErrors) =>
    touched && errors[key] ? (
      <span className="error" style={{ fontSize: '12px', display: 'block' }}>
        {errors[key]}
      </span>
    ) : null;

  return (
    <form onSubmit={handleSubmit} noValidate>
      <div style={{ marginBottom: '0.75rem' }}>
        <label htmlFor="hotl-scope">
          <strong>Scope</strong> <span style={{ color: 'var(--danger)' }}>*</span>
        </label>
        <input
          id="hotl-scope"
          type="text"
          list="hotl-scope-list"
          value={form.scope}
          onChange={(e) => set('scope', e.target.value)}
          placeholder="llm_call"
          disabled={saving}
          className="search"
          style={{ width: '100%' }}
        />
        <datalist id="hotl-scope-list">
          {KNOWN_SCOPES.map((s) => (
            <option key={s} value={s} />
          ))}
        </datalist>
        {showErr('scope')}
      </div>

      <div style={{ marginBottom: '0.75rem' }}>
        <label htmlFor="hotl-window">
          <strong>Window (seconds)</strong> <span style={{ color: 'var(--danger)' }}>*</span>
        </label>
        <input
          id="hotl-window"
          type="number"
          min={1}
          step={1}
          value={form.window_seconds}
          onChange={(e) => set('window_seconds', e.target.value)}
          placeholder="3600"
          disabled={saving}
          className="search"
          style={{ width: '100%' }}
        />
        {showErr('window_seconds')}
      </div>

      {touched && errors.limits && (
        <div className="error" style={{ fontSize: '12px', marginBottom: '0.5rem' }}>
          {errors.limits}
        </div>
      )}

      <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '0.75rem', marginBottom: '0.75rem' }}>
        <div>
          <label htmlFor="hotl-max-count">Max count</label>
          <input
            id="hotl-max-count"
            type="number"
            min={1}
            step={1}
            value={form.max_count}
            onChange={(e) => set('max_count', e.target.value)}
            placeholder="100"
            disabled={saving}
            className="search"
            style={{ width: '100%' }}
          />
          {showErr('max_count')}
        </div>
        <div>
          <label htmlFor="hotl-max-usd">Max USD</label>
          <input
            id="hotl-max-usd"
            type="number"
            min={0}
            step={0.01}
            value={form.max_usd}
            onChange={(e) => set('max_usd', e.target.value)}
            placeholder="5.00"
            disabled={saving}
            className="search"
            style={{ width: '100%' }}
          />
          {showErr('max_usd')}
        </div>
      </div>

      <div style={{ marginBottom: '1rem' }}>
        <label htmlFor="hotl-escalate">Escalate to (optional)</label>
        <input
          id="hotl-escalate"
          type="text"
          value={form.escalate_to}
          onChange={(e) => set('escalate_to', e.target.value)}
          placeholder="ops@example.com or #channel"
          disabled={saving}
          className="search"
          style={{ width: '100%' }}
        />
        <span style={{ fontSize: '12px', color: 'var(--muted)' }}>
          Tier presets: tier-1, tier-2, tier-3 — or enter any email / IM destination.
          Leave empty to deny on breach.
        </span>
      </div>

      {serverError && (
        <div className="error" style={{ marginBottom: '0.75rem' }}>
          {serverError}
        </div>
      )}

      <div style={{ display: 'flex', gap: '0.5rem', justifyContent: 'flex-end' }}>
        <button type="button" onClick={onCancel} disabled={saving} className="skill-cancel-btn">
          Cancel
        </button>
        <button type="submit" disabled={saving || (touched && hasErrors)} className="skill-install-btn">
          {saving ? 'Saving…' : submitLabel}
        </button>
      </div>
    </form>
  );
}

// ---------------------------------------------------------------------------
// Create / Edit modal
// ---------------------------------------------------------------------------

interface PolicyModalProps {
  modal: Exclude<ModalState, { kind: 'closed' }>;
  onClose: () => void;
  onSaved: (policy: HotlPolicy) => void;
}

function PolicyModal({ modal, onClose, onSaved }: PolicyModalProps): JSX.Element {
  const [saving, setSaving] = useState(false);
  const [serverError, setServerError] = useState<string | null>(null);
  const closeRef = useRef<HTMLButtonElement>(null);

  const initial = modal.kind === 'edit' ? policyToForm(modal.policy) : EMPTY_FORM;
  const title = modal.kind === 'create' ? 'New policy' : 'Edit policy';
  const submitLabel = modal.kind === 'create' ? 'Create' : 'Save';

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('keydown', handler);
    closeRef.current?.focus();
    return () => document.removeEventListener('keydown', handler);
  }, [onClose]);

  async function handleSubmit(f: FormState) {
    setSaving(true);
    setServerError(null);
    try {
      const req = formToRequest(f);
      let saved: HotlPolicy;
      if (modal.kind === 'edit') {
        saved = await client.updateHotlPolicy(modal.policy.id, req);
      } else {
        saved = await client.createHotlPolicy(req);
      }
      onSaved(saved);
    } catch (e) {
      setServerError((e as Error).message);
    } finally {
      setSaving(false);
    }
  }

  return (
    <aside className="skill-drawer" role="dialog" aria-modal="true" aria-label={title}>
      <div className="skill-drawer-overlay" onClick={onClose} aria-hidden="true" />
      <div className="skill-drawer-panel">
        <header className="skill-drawer-header">
          <h2 className="skill-drawer-title">{title}</h2>
          <button
            ref={closeRef}
            className="skill-drawer-close"
            onClick={onClose}
            aria-label="Close modal"
          >
            ✕
          </button>
        </header>
        <div className="skill-drawer-body">
          <PolicyForm
            initial={initial}
            submitLabel={submitLabel}
            onSubmit={(f) => void handleSubmit(f)}
            onCancel={onClose}
            saving={saving}
            serverError={serverError}
          />
        </div>
      </div>
    </aside>
  );
}

// ---------------------------------------------------------------------------
// Delete confirmation
// ---------------------------------------------------------------------------

interface DeleteDialogProps {
  state: Extract<DeleteState, { kind: 'confirming' }>;
  onConfirm: () => void;
  onCancel: () => void;
  deleting: boolean;
}

function DeleteDialog({ state, onConfirm, onCancel, deleting }: DeleteDialogProps): JSX.Element {
  return (
    <div className="skill-confirm" role="dialog" aria-modal="true" aria-label="Confirm delete">
      <p>
        Delete policy for scope <strong>{state.scope}</strong>? This cannot be undone.
      </p>
      <div className="skill-confirm-actions">
        <button
          onClick={onConfirm}
          disabled={deleting}
          className="skill-install-btn"
          style={{ background: 'var(--danger, #dc2626)' }}
        >
          {deleting ? 'Deleting…' : 'Delete'}
        </button>
        <button onClick={onCancel} disabled={deleting} className="skill-cancel-btn">
          Cancel
        </button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Test policy drawer
// ---------------------------------------------------------------------------

interface TestDrawerProps {
  onClose: () => void;
}

function TestDrawer({ onClose }: TestDrawerProps): JSX.Element {
  const [scope, setScope] = useState('');
  const [amount, setAmount] = useState('1.0');
  const [checkState, setCheckState] = useState<CheckState>({ kind: 'idle' });
  const closeRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('keydown', handler);
    closeRef.current?.focus();
    return () => document.removeEventListener('keydown', handler);
  }, [onClose]);

  async function handleCheck(e: React.FormEvent) {
    e.preventDefault();
    const req: HotlCheckRequest = {
      scope: scope.trim(),
      amount: Number(amount),
    };
    setCheckState({ kind: 'running' });
    try {
      const verdict = await client.checkHotlPolicy(req);
      setCheckState({ kind: 'result', verdict });
    } catch (err) {
      setCheckState({ kind: 'error', message: (err as Error).message });
    }
  }

  const verdictColor = (v: string) => {
    if (v === 'allow') return '#22c55e';
    if (v === 'escalate') return '#f59e0b';
    return '#dc2626';
  };

  return (
    <aside className="skill-drawer" role="complementary" aria-label="Test policy">
      <div className="skill-drawer-overlay" onClick={onClose} aria-hidden="true" />
      <div className="skill-drawer-panel">
        <header className="skill-drawer-header">
          <h2 className="skill-drawer-title">Test policy</h2>
          <button
            ref={closeRef}
            className="skill-drawer-close"
            onClick={onClose}
            aria-label="Close test drawer"
          >
            ✕
          </button>
        </header>
        <div className="skill-drawer-body">
          <p style={{ fontSize: '13px', color: 'var(--muted)', marginBottom: '1rem' }}>
            Calls <code>POST /v1/hotl/check</code>. This records the action in the
            usage log — use a test scope to avoid polluting production data.
          </p>
          <form onSubmit={(e) => void handleCheck(e)}>
            <div style={{ marginBottom: '0.75rem' }}>
              <label htmlFor="test-scope">Scope</label>
              <input
                id="test-scope"
                type="text"
                list="test-scope-list"
                value={scope}
                onChange={(e) => setScope(e.target.value)}
                className="search"
                style={{ width: '100%' }}
                placeholder="llm_call"
                required
              />
              <datalist id="test-scope-list">
                {KNOWN_SCOPES.map((s) => (
                  <option key={s} value={s} />
                ))}
              </datalist>
            </div>
            <div style={{ marginBottom: '1rem' }}>
              <label htmlFor="test-amount">Amount</label>
              <input
                id="test-amount"
                type="number"
                min={0}
                step={0.001}
                value={amount}
                onChange={(e) => setAmount(e.target.value)}
                className="search"
                style={{ width: '100%' }}
                required
              />
              <span style={{ fontSize: '12px', color: 'var(--muted)' }}>
                Use 1.0 for invocation counting; USD cost for cost-budget scopes.
              </span>
            </div>
            <button
              type="submit"
              className="skill-install-btn"
              disabled={checkState.kind === 'running' || !scope.trim()}
            >
              {checkState.kind === 'running' ? 'Checking…' : 'Check'}
            </button>
          </form>

          {checkState.kind === 'result' && (
            <div
              style={{
                marginTop: '1rem',
                padding: '0.75rem',
                borderLeft: `4px solid ${verdictColor(checkState.verdict.verdict)}`,
                background: 'var(--surface-2, #f9fafb)',
                borderRadius: '4px',
              }}
              aria-live="polite"
              aria-label="Check result"
            >
              <strong style={{ color: verdictColor(checkState.verdict.verdict), textTransform: 'uppercase' }}>
                {checkState.verdict.verdict}
              </strong>
              {checkState.verdict.reason && (
                <p style={{ fontSize: '13px', marginTop: '0.25rem', color: 'var(--muted)' }}>
                  {checkState.verdict.reason}
                </p>
              )}
            </div>
          )}

          {checkState.kind === 'error' && (
            <div className="error" style={{ marginTop: '1rem' }} aria-live="polite">
              {checkState.message}
            </div>
          )}
        </div>
      </div>
    </aside>
  );
}

// ---------------------------------------------------------------------------
// Main pane
// ---------------------------------------------------------------------------

export function HotlPoliciesPane(): JSX.Element {
  const { t } = useTranslation();
  const [loadState, setLoadState] = useState<LoadState>({ kind: 'loading' });
  const [modal, setModal] = useState<ModalState>({ kind: 'closed' });
  const [deleteState, setDeleteState] = useState<DeleteState>({ kind: 'idle' });
  const [showTestDrawer, setShowTestDrawer] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoadState({ kind: 'loading' });
    try {
      // Single owner (DEC-033): list every policy — no scope filter.
      const policies = await client.listHotlPolicies();
      setLoadState({ kind: 'ok', policies });
    } catch (err) {
      if (is503(err)) {
        setLoadState({ kind: 'unavailable' });
      } else {
        setLoadState({ kind: 'error', message: (err as Error).message });
      }
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  function handleSaved(policy: HotlPolicy) {
    setModal({ kind: 'closed' });
    setSaveError(null);
    setLoadState((prev) => {
      if (prev.kind === 'ok') {
        const existing = prev.policies.findIndex((p) => p.id === policy.id);
        if (existing >= 0) {
          const updated = prev.policies.map((p) => (p.id === policy.id ? policy : p));
          return { kind: 'ok', policies: updated };
        }
        return { kind: 'ok', policies: [...prev.policies, policy] };
      }
      return { kind: 'ok', policies: [policy] };
    });
  }

  async function handleDeleteConfirm() {
    if (deleteState.kind !== 'confirming') return;
    const { id } = deleteState;
    setDeleteState({ kind: 'deleting' });
    try {
      await client.deleteHotlPolicy(id);
      setDeleteState({ kind: 'idle' });
      setLoadState((prev) => {
        if (prev.kind !== 'ok') return prev;
        return { kind: 'ok', policies: prev.policies.filter((p) => p.id !== id) };
      });
    } catch (e) {
      setSaveError((e as Error).message);
      setDeleteState({ kind: 'idle' });
    }
  }

  const policies = loadState.kind === 'ok' ? loadState.policies : [];

  return (
    <>
      <header className="today-header">
        <h1>{t('pane.hotl_policies.title')}</h1>
        <div className="today-meta">
          <button
            onClick={() => void load()}
            disabled={loadState.kind === 'loading'}
          >
            {loadState.kind === 'loading' ? t('common.loading') : t('common.refresh')}
          </button>
          <button
            className="skill-install-btn"
            onClick={() => setShowTestDrawer(true)}
            style={{ marginLeft: '0.5rem' }}
          >
            {t('pane.hotl_policies.btn_test')}
          </button>
          <button
            className="skill-install-btn"
            onClick={() => setModal({ kind: 'create' })}
            style={{ marginLeft: '0.5rem' }}
          >
            {t('pane.hotl_policies.btn_new')}
          </button>
        </div>
      </header>

      <PaneIntro
        purpose={t('pane.hotl_policies.intro.purpose')}
        usage={t('pane.hotl_policies.intro.usage')}
        usageLabel={t('pane.hotl_policies.intro.usage_label')}
        examplesLabel={t('pane.hotl_policies.intro.examples_label')}
        examples={[
          t('pane.hotl_policies.intro.example_1'),
          t('pane.hotl_policies.intro.example_2'),
        ]}
      />

      {/* 503 graceful banner */}
      {loadState.kind === 'unavailable' && (
        <div
          className="skill-activation-notice"
          role="alert"
          aria-label="Store unavailable"
        >
          <strong>{t('pane.hotl_policies.unavailable_title')}</strong>{' '}
          {t('pane.hotl_policies.unavailable_body')}{' '}
          <a href="/bridges-status" rel="noopener noreferrer">
            {t('pane.hotl_policies.unavailable_link')}
          </a>
        </div>
      )}

      {/* Generic error */}
      {loadState.kind === 'error' && (
        <div className="error">
          {t('common.failed', { message: loadState.message })}
        </div>
      )}

      {/* Save/delete error */}
      {saveError && (
        <div className="error">{saveError}</div>
      )}

      {/* Delete confirmation */}
      {deleteState.kind === 'confirming' && (
        <DeleteDialog
          state={deleteState}
          onConfirm={() => void handleDeleteConfirm()}
          onCancel={() => setDeleteState({ kind: 'idle' })}
          deleting={false}
        />
      )}
      {deleteState.kind === 'deleting' && (
        <DeleteDialog
          state={{ kind: 'confirming', id: '', scope: '' }}
          onConfirm={() => { /* noop while deleting */ }}
          onCancel={() => { /* noop while deleting */ }}
          deleting={true}
        />
      )}

      {/* Loading */}
      {loadState.kind === 'loading' && (
        <div className="empty">{t('common.loading')}</div>
      )}

      {/* Empty state */}
      {loadState.kind === 'ok' && policies.length === 0 && (
        <div className="empty">
          {t('pane.hotl_policies.empty')}{' '}
          <button
            className="skill-install-btn"
            onClick={() => setModal({ kind: 'create' })}
            style={{ marginLeft: '0.5rem' }}
          >
            {t('pane.hotl_policies.btn_new')}
          </button>
        </div>
      )}

      {/* Graduated-trust overview (P3, DEC roadmap) */}
      {loadState.kind === 'ok' && policies.length > 0 && <TrustTiers policies={policies} />}

      {/* Policies table */}
      {loadState.kind === 'ok' && policies.length > 0 && (
        <section aria-label="HotL policies">
          <table className="usage-table">
            <thead>
              <tr>
                <th scope="col">{t('pane.hotl_policies.col_scope')}</th>
                <th scope="col">{t('pane.hotl_policies.col_max_count')}</th>
                <th scope="col">{t('pane.hotl_policies.col_max_usd')}</th>
                <th scope="col">{t('pane.hotl_policies.col_window')}</th>
                <th scope="col">{t('pane.hotl_policies.col_escalate_to')}</th>
                <th scope="col" aria-label="Actions" />
              </tr>
            </thead>
            <tbody>
              {policies.map((p) => (
                <tr key={p.id}>
                  <td>
                    <code>{p.scope}</code>
                  </td>
                  <td>{p.max_count !== null ? p.max_count : '—'}</td>
                  <td>{p.max_usd !== null ? `$${p.max_usd.toFixed(2)}` : '—'}</td>
                  <td>{fmtWindow(p.window_seconds)}</td>
                  <td style={{ fontSize: '12px', color: p.escalate_to ? undefined : 'var(--muted)' }}>
                    {p.escalate_to ?? 'deny on breach'}
                  </td>
                  <td>
                    <button
                      className="skill-detail-btn"
                      onClick={() => setModal({ kind: 'edit', policy: p })}
                      aria-label={`Edit policy for ${p.scope}`}
                    >
                      {t('pane.hotl_policies.btn_edit')}
                    </button>
                    <button
                      className="skill-cancel-btn"
                      onClick={() => {
                        setSaveError(null);
                        setDeleteState({ kind: 'confirming', id: p.id, scope: p.scope });
                      }}
                      aria-label={`Delete policy for ${p.scope}`}
                      style={{ marginLeft: '0.25rem' }}
                    >
                      {t('pane.hotl_policies.btn_delete')}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}

      {/* Create / Edit modal */}
      {modal.kind !== 'closed' && (
        <PolicyModal
          modal={modal}
          onClose={() => { setModal({ kind: 'closed' }); setSaveError(null); }}
          onSaved={handleSaved}
        />
      )}

      {/* Test drawer */}
      {showTestDrawer && (
        <TestDrawer
          onClose={() => setShowTestDrawer(false)}
        />
      )}
    </>
  );
}
