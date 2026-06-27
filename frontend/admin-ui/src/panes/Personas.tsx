/**
 * v1.8.0 (sprint-10b S10b-2) — Personas pane.
 *
 * CRUD against `/v1/personas` (mounted by S10b-1 Phase A). The pane:
 *   - Shows a filterable table of personas (single owner — no scope axis).
 *   - Colour-codes the role tag (sprint-9 DEC-021 triangle): planner /
 *     worker / critic. The persona's role is inferred from a
 *     `role/<name>` token in the system prompt (the DTO has no
 *     dedicated tag field; see Persona type in @xiaoguai/shared).
 *   - Opens a side drawer for create/edit.
 *   - Gates mutating buttons behind `<RequireScope name="personas.write">`.
 *   - Renders Loading / Error / Empty / Unavailable (503) / Ready
 *     states per LLD-ADMIN-UI-001 §4.1.
 *
 * Memory view scope / role tags: the underlying DTO does not yet carry a
 * `tags` field (see crates/xiaoguai-personas/src/model.rs). Until that
 * lands, we infer the role from the system prompt — any of the strings
 * `role/planner`, `role/worker`, `role/critic` will colour the chip.
 * If we don't find a token, the chip renders as "—" so operators see
 * at a glance which personas are still un-tagged.
 */

import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type {
  CreatePersonaRequest,
  Persona,
  UpdatePersonaRequest,
  XiaoguaiClient,
} from '@xiaoguai/shared';
import { ApiError } from '@xiaoguai/shared';
import { client as defaultClient } from '../client';
import { RequireScope } from '../components/RequireScope';
import { PaneIntro } from '../components/PaneIntro';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type RoleTag = 'planner' | 'worker' | 'critic' | null;

type LoadState =
  | { kind: 'loading' }
  | { kind: 'ok'; personas: Persona[] }
  | { kind: 'unavailable' }
  | { kind: 'error'; message: string };

type DrawerState =
  | { kind: 'closed' }
  | { kind: 'create' }
  | { kind: 'edit'; persona: Persona };

type DeleteState =
  | { kind: 'idle' }
  | { kind: 'confirming'; persona: Persona }
  | { kind: 'deleting' };

interface FormState {
  name: string;
  system_prompt: string;
  default_model: string;
  escalation_tier: string;
  tool_allowlist_csv: string;
}

const EMPTY_FORM: FormState = {
  name: '',
  system_prompt: '',
  default_model: '',
  escalation_tier: '',
  tool_allowlist_csv: '',
};

// ---------------------------------------------------------------------------
// Pure helpers (testable without DOM)
// ---------------------------------------------------------------------------

const ROLE_PATTERN = /role\/(planner|worker|critic)/i;

export function inferRoleTag(persona: Persona): RoleTag {
  const match = persona.system_prompt.match(ROLE_PATTERN);
  if (!match || !match[1]) return null;
  return match[1].toLowerCase() as RoleTag;
}

export function roleClassName(role: RoleTag): string {
  // Mirrors the kind-tag-* convention from Today / Memory.
  if (role === 'planner') return 'kind-tag kind-tag-chat';
  if (role === 'worker') return 'kind-tag kind-tag-scheduled';
  if (role === 'critic') return 'kind-tag kind-tag-im';
  return 'kind-tag';
}

export function filterPersonas(
  personas: Persona[],
  nameFilter: string,
  roleFilter: RoleTag | 'all',
): Persona[] {
  const lower = nameFilter.trim().toLowerCase();
  return personas.filter((p) => {
    if (lower && !p.name.toLowerCase().includes(lower)) return false;
    if (roleFilter !== 'all') {
      const role = inferRoleTag(p);
      if (role !== roleFilter) return false;
    }
    return true;
  });
}

export function fmtDate(iso: string): string {
  try {
    return new Date(iso).toLocaleString(undefined, {
      month: 'short',
      day: 'numeric',
      year: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  } catch {
    return iso;
  }
}

export function personaToForm(p: Persona): FormState {
  return {
    name: p.name,
    system_prompt: p.system_prompt,
    default_model: p.default_model ?? '',
    escalation_tier: p.escalation_tier ?? '',
    tool_allowlist_csv: (p.tool_allowlist ?? []).join(', '),
  };
}

export function formToCreateReq(f: FormState): CreatePersonaRequest {
  return {
    name: f.name.trim(),
    system_prompt: f.system_prompt,
    default_model: f.default_model.trim() === '' ? null : f.default_model.trim(),
    escalation_tier:
      f.escalation_tier.trim() === '' ? null : f.escalation_tier.trim(),
    tool_allowlist: parseAllowlist(f.tool_allowlist_csv),
  };
}

export function formToUpdateReq(f: FormState): UpdatePersonaRequest {
  return {
    name: f.name.trim(),
    system_prompt: f.system_prompt,
    default_model: f.default_model.trim() === '' ? null : f.default_model.trim(),
    escalation_tier:
      f.escalation_tier.trim() === '' ? null : f.escalation_tier.trim(),
    tool_allowlist: parseAllowlist(f.tool_allowlist_csv),
  };
}

function parseAllowlist(csv: string): string[] | null {
  const trimmed = csv.trim();
  if (trimmed === '') return null;
  return trimmed
    .split(',')
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export interface PersonasPaneProps {
  /** Override the shared client (used by tests). */
  client?: Pick<
    XiaoguaiClient,
    'listPersonas' | 'createPersona' | 'updatePersona' | 'deletePersona'
  >;
}

export function PersonasPane({ client }: PersonasPaneProps = {}): JSX.Element {
  const c = client ?? defaultClient;
  const { t } = useTranslation();

  const [load, setLoad] = useState<LoadState>({ kind: 'loading' });
  const [nameFilter, setNameFilter] = useState('');
  const [roleFilter, setRoleFilter] = useState<RoleTag | 'all'>('all');
  const [drawer, setDrawer] = useState<DrawerState>({ kind: 'closed' });
  const [del, setDel] = useState<DeleteState>({ kind: 'idle' });
  const [form, setForm] = useState<FormState>(EMPTY_FORM);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoad({ kind: 'loading' });
    try {
      const personas = await c.listPersonas();
      setLoad({ kind: 'ok', personas });
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
  }, [c]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const filtered = useMemo(() => {
    if (load.kind !== 'ok') return [];
    return filterPersonas(load.personas, nameFilter, roleFilter);
  }, [load, nameFilter, roleFilter]);

  function openCreate() {
    setForm(EMPTY_FORM);
    setSaveError(null);
    setDrawer({ kind: 'create' });
  }

  function openEdit(p: Persona) {
    setForm(personaToForm(p));
    setSaveError(null);
    setDrawer({ kind: 'edit', persona: p });
  }

  function closeDrawer() {
    setDrawer({ kind: 'closed' });
    setSaveError(null);
  }

  async function onSave() {
    if (drawer.kind === 'closed') return;
    setSaving(true);
    setSaveError(null);
    try {
      if (drawer.kind === 'create') {
        await c.createPersona(formToCreateReq(form));
      } else {
        await c.updatePersona(drawer.persona.id, formToUpdateReq(form));
      }
      setDrawer({ kind: 'closed' });
      await refresh();
    } catch (err) {
      setSaveError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  }

  async function onConfirmDelete() {
    if (del.kind !== 'confirming') return;
    const target = del.persona;
    setDel({ kind: 'deleting' });
    try {
      await c.deletePersona(target.id);
      setDel({ kind: 'idle' });
      await refresh();
    } catch (err) {
      setDel({ kind: 'idle' });
      setLoad({
        kind: 'error',
        message: err instanceof Error ? err.message : String(err),
      });
    }
  }

  // -- render --------------------------------------------------------------

  return (
    <section aria-labelledby="personas-title" className="pane">
      <header>
        <h1 id="personas-title">{t('pane.personas.title')}</h1>
      </header>

      <PaneIntro
        purpose={t('pane.personas.intro.purpose')}
        usage={t('pane.personas.intro.usage')}
        usageLabel={t('pane.personas.intro.usage_label')}
      />

      <div className="toolbar" role="search" aria-label="personas filters">
        <input
          type="search"
          value={nameFilter}
          placeholder={t('pane.personas.filter_name_placeholder')}
          onChange={(e) => setNameFilter(e.target.value)}
          aria-label={t('pane.personas.filter_name_placeholder')}
        />
        <select
          value={roleFilter ?? 'all'}
          onChange={(e) => setRoleFilter(e.target.value as RoleTag | 'all')}
          aria-label="role filter"
        >
          <option value="all">{t('pane.personas.filter_tag_all')}</option>
          <option value="planner">{t('pane.personas.filter_tag_planner')}</option>
          <option value="worker">{t('pane.personas.filter_tag_worker')}</option>
          <option value="critic">{t('pane.personas.filter_tag_critic')}</option>
        </select>
        <RequireScope name="personas.write">
          <button type="button" onClick={openCreate}>
            {t('pane.personas.btn_new')}
          </button>
        </RequireScope>
      </div>

      {load.kind === 'loading' && (
        <p role="status">{t('pane.personas.loading')}</p>
      )}
      {load.kind === 'unavailable' && (
        <p role="alert" className="alert">
          {t('pane.personas.unavailable')}
        </p>
      )}
      {load.kind === 'error' && (
        <p role="alert" className="alert">
          {t('common.failed', { message: load.message })}
        </p>
      )}
      {load.kind === 'ok' && filtered.length === 0 && (
        <p role="status" className="muted">
          {t('pane.personas.empty')}
        </p>
      )}
      {load.kind === 'ok' && filtered.length > 0 && (
        <table aria-label="personas">
          <thead>
            <tr>
              <th>{t('pane.personas.col_name')}</th>
              <th>{t('pane.personas.col_tags')}</th>
              <th>{t('pane.personas.col_model')}</th>
              <th>{t('pane.personas.col_created')}</th>
              <th>{t('pane.personas.col_actions')}</th>
            </tr>
          </thead>
          <tbody>
            {filtered.map((p) => {
              const role = inferRoleTag(p);
              return (
                <tr key={p.id}>
                  <td>{p.name}</td>
                  <td>
                    <span className={roleClassName(role)}>{role ?? '—'}</span>
                  </td>
                  <td>{p.default_model ?? '—'}</td>
                  <td>{fmtDate(p.created_at)}</td>
                  <td>
                    <RequireScope name="personas.write">
                      <button
                        type="button"
                        onClick={() => openEdit(p)}
                        aria-label={`edit ${p.name}`}
                      >
                        {t('pane.personas.btn_edit')}
                      </button>
                    </RequireScope>{' '}
                    <RequireScope name="personas.delete">
                      <button
                        type="button"
                        onClick={() =>
                          setDel({ kind: 'confirming', persona: p })
                        }
                        aria-label={`delete ${p.name}`}
                      >
                        {t('pane.personas.btn_delete')}
                      </button>
                    </RequireScope>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}

      {drawer.kind !== 'closed' && (
        <div className="drawer-backdrop" role="dialog" aria-modal="true">
          <div className="drawer">
            <div className="drawer-header">
              <h2>
                {drawer.kind === 'create'
                  ? t('pane.personas.drawer_create_title')
                  : t('pane.personas.drawer_edit_title')}
              </h2>
              <button
                type="button"
                className="drawer-close"
                onClick={closeDrawer}
                aria-label={t('common.close')}
              >
                ×
              </button>
            </div>
            <form
              onSubmit={(e) => {
                e.preventDefault();
                void onSave();
              }}
            >
              <label>
                <span>{t('pane.personas.field_name')}</span>
                <input
                  type="text"
                  value={form.name}
                  placeholder={t('pane.personas.placeholder_name')}
                  onChange={(e) =>
                    setForm({ ...form, name: e.target.value })
                  }
                  required
                />
              </label>
              <label>
                <span>{t('pane.personas.field_system_prompt')}</span>
                <textarea
                  rows={6}
                  value={form.system_prompt}
                  placeholder={t('pane.personas.placeholder_system_prompt')}
                  onChange={(e) =>
                    setForm({ ...form, system_prompt: e.target.value })
                  }
                />
              </label>
              <label>
                <span>{t('pane.personas.field_model')}</span>
                <input
                  type="text"
                  value={form.default_model}
                  onChange={(e) =>
                    setForm({ ...form, default_model: e.target.value })
                  }
                />
              </label>
              <label>
                <span>{t('pane.personas.field_escalation_tier')}</span>
                <input
                  type="text"
                  value={form.escalation_tier}
                  onChange={(e) =>
                    setForm({ ...form, escalation_tier: e.target.value })
                  }
                />
              </label>
              <label>
                <span>{t('pane.personas.field_tool_allowlist')}</span>
                <input
                  type="text"
                  value={form.tool_allowlist_csv}
                  placeholder={t('pane.personas.placeholder_tool_allowlist')}
                  onChange={(e) =>
                    setForm({ ...form, tool_allowlist_csv: e.target.value })
                  }
                />
              </label>
              {saveError && (
                <p role="alert" className="alert">
                  {saveError}
                </p>
              )}
              <div className="drawer-actions">
                <button type="button" onClick={closeDrawer}>
                  {t('pane.personas.btn_cancel')}
                </button>
                <button type="submit" disabled={saving}>
                  {t('pane.personas.btn_save')}
                </button>
              </div>
            </form>
          </div>
        </div>
      )}

      {del.kind === 'confirming' && (
        <div className="drawer-backdrop" role="dialog" aria-modal="true">
          <div className="drawer">
            <div className="drawer-header">
              <h2>{t('pane.personas.delete_confirm_title')}</h2>
            </div>
            <p>{t('pane.personas.delete_confirm_body')}</p>
            <p>
              <strong>{del.persona.name}</strong>
            </p>
            <div className="drawer-actions">
              <button type="button" onClick={() => setDel({ kind: 'idle' })}>
                {t('pane.personas.btn_cancel')}
              </button>
              <button
                type="button"
                onClick={() => {
                  void onConfirmDelete();
                }}
              >
                {t('pane.personas.btn_delete')}
              </button>
            </div>
          </div>
        </div>
      )}
    </section>
  );
}
