/**
 * T3.6 (expert center, 2026-06-10 §2.4) — Expert Teams pane.
 *
 * CRUD against `/v1/teams`. The pane:
 *   - Shows a filterable table of teams (name search).
 *   - Resolves the lead persona's name via the loaded personas list
 *     (falls back to the raw uuid when not found).
 *   - Opens a side drawer for create/edit: members are a checkbox
 *     multi-select over active personas (with the sprint-9 role chip,
 *     reusing inferRoleTag/roleClassName from the Personas pane); the
 *     lead select is restricted to the chosen members.
 *   - Pack slugs are display-only tags (decision ③A) entered as a
 *     comma-separated string.
 *   - Gates mutating buttons behind `<RequireScope name="teams.write">`
 *     / `"teams.delete"` (single-owner fail-open: scopes are free-form
 *     client-side strings, see hooks/useScopes.tsx).
 *   - Renders Loading / Error / Empty / Unavailable (503) / Ready
 *     states per LLD-ADMIN-UI-001 §4.1.
 *
 * Client-side guard: at least one member, and the lead must be among
 * the members — save is disabled with a hint otherwise. The server
 * re-validates anyway (400 lead-not-member / missing-or-archived
 * personas; 409 duplicate name).
 */

import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type {
  CreateTeamRequest,
  Persona,
  Team,
  UpdateTeamRequest,
  XiaoguaiClient,
} from '@xiaoguai/shared';
import { ApiError } from '@xiaoguai/shared';
import { client as defaultClient } from '../client';
import { RequireScope } from '../components/RequireScope';
import { fmtDate, inferRoleTag, roleClassName } from './Personas';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type LoadState =
  | { kind: 'loading' }
  | { kind: 'ok'; teams: Team[]; personas: Persona[] }
  | { kind: 'unavailable' }
  | { kind: 'error'; message: string };

type DrawerState =
  | { kind: 'closed' }
  | { kind: 'create' }
  | { kind: 'edit'; team: Team };

type DeleteState =
  | { kind: 'idle' }
  | { kind: 'confirming'; team: Team }
  | { kind: 'deleting' };

export interface TeamFormState {
  name: string;
  description: string;
  member_persona_ids: string[];
  lead_persona_id: string;
  pack_slugs_csv: string;
  /** T7.1 — glossary markdown. `''` in the form maps to "no glossary". */
  glossary_md: string;
}

export const EMPTY_TEAM_FORM: TeamFormState = {
  name: '',
  description: '',
  member_persona_ids: [],
  lead_persona_id: '',
  pack_slugs_csv: '',
  glossary_md: '',
};

/**
 * T7.1 — server-side byte cap on the glossary (mirrors
 * `xiaoguai_personas::teams::model::MAX_GLOSSARY_BYTES`). The drawer shows
 * a soft warning when over; the server rejects with 400 and the existing
 * save-error path surfaces it.
 */
export const MAX_GLOSSARY_BYTES = 16_384;

/** UTF-8 byte length of the glossary text (the cap is in bytes, not chars). */
export function glossaryByteLength(text: string): number {
  return new TextEncoder().encode(text).length;
}

/** Validation problems the drawer can surface (i18n keys derive from these). */
export type TeamFormProblem = 'no_members' | 'lead_not_member' | null;

// ---------------------------------------------------------------------------
// Pure helpers (testable without DOM)
// ---------------------------------------------------------------------------

export function teamToForm(team: Team): TeamFormState {
  return {
    name: team.name,
    description: team.description,
    member_persona_ids: [...team.member_persona_ids],
    lead_persona_id: team.lead_persona_id,
    pack_slugs_csv: team.recommended_pack_slugs.join(', '),
    glossary_md: team.glossary_md ?? '',
  };
}

export function parsePackSlugs(csv: string): string[] {
  return csv
    .split(',')
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

export function formToCreateTeamReq(f: TeamFormState): CreateTeamRequest {
  return {
    name: f.name.trim(),
    description: f.description.trim(),
    lead_persona_id: f.lead_persona_id,
    member_persona_ids: [...f.member_persona_ids],
    recommended_pack_slugs: parsePackSlugs(f.pack_slugs_csv),
    // Verbatim: the server normalises blank/whitespace-only to "no glossary".
    glossary_md: f.glossary_md,
  };
}

export function formToUpdateTeamReq(f: TeamFormState): UpdateTeamRequest {
  return {
    name: f.name.trim(),
    description: f.description.trim(),
    lead_persona_id: f.lead_persona_id,
    member_persona_ids: [...f.member_persona_ids],
    recommended_pack_slugs: parsePackSlugs(f.pack_slugs_csv),
    // Verbatim — NOT null: on update, `null`/omitted means "leave unchanged"
    // while a blank string CLEARS the glossary. The form is seeded from the
    // team, so an untouched value round-trips unchanged.
    glossary_md: f.glossary_md,
  };
}

/**
 * Client-side guard mirroring the backend invariants: at least one
 * member, and the lead must be one of the members.
 */
export function validateTeamForm(f: TeamFormState): TeamFormProblem {
  if (f.member_persona_ids.length === 0) return 'no_members';
  if (
    f.lead_persona_id === '' ||
    !f.member_persona_ids.includes(f.lead_persona_id)
  ) {
    return 'lead_not_member';
  }
  return null;
}

/** Immutable membership toggle: returns a NEW id list. */
export function toggleMember(ids: readonly string[], id: string): string[] {
  return ids.includes(id) ? ids.filter((x) => x !== id) : [...ids, id];
}

export function filterTeams(teams: Team[], nameFilter: string): Team[] {
  const lower = nameFilter.trim().toLowerCase();
  if (lower === '') return teams;
  return teams.filter((t) => t.name.toLowerCase().includes(lower));
}

/** Resolve a persona id to its name; fall back to the raw uuid. */
export function resolvePersonaName(personas: Persona[], id: string): string {
  return personas.find((p) => p.id === id)?.name ?? id;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export interface ExpertTeamsPaneProps {
  /** Override the shared client (used by tests). */
  client?: Pick<
    XiaoguaiClient,
    'listPersonas' | 'listTeams' | 'createTeam' | 'updateTeam' | 'deleteTeam'
  >;
}

export function ExpertTeamsPane({
  client,
}: ExpertTeamsPaneProps = {}): JSX.Element {
  const c = client ?? defaultClient;
  const { t } = useTranslation();

  const [load, setLoad] = useState<LoadState>({ kind: 'loading' });
  const [nameFilter, setNameFilter] = useState('');
  const [drawer, setDrawer] = useState<DrawerState>({ kind: 'closed' });
  const [del, setDel] = useState<DeleteState>({ kind: 'idle' });
  const [form, setForm] = useState<TeamFormState>(EMPTY_TEAM_FORM);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoad({ kind: 'loading' });
    try {
      const [teams, personas] = await Promise.all([
        c.listTeams(),
        c.listPersonas(),
      ]);
      setLoad({ kind: 'ok', teams, personas });
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
    return filterTeams(load.teams, nameFilter);
  }, [load, nameFilter]);

  const personas = load.kind === 'ok' ? load.personas : [];
  const activePersonas = useMemo(
    () => personas.filter((p) => !p.archived),
    [personas],
  );

  const problem = validateTeamForm(form);

  function openCreate() {
    setForm(EMPTY_TEAM_FORM);
    setSaveError(null);
    setDrawer({ kind: 'create' });
  }

  function openEdit(team: Team) {
    setForm(teamToForm(team));
    setSaveError(null);
    setDrawer({ kind: 'edit', team });
  }

  function closeDrawer() {
    setDrawer({ kind: 'closed' });
    setSaveError(null);
  }

  function onToggleMember(id: string) {
    setForm((f) => ({
      ...f,
      member_persona_ids: toggleMember(f.member_persona_ids, id),
    }));
  }

  async function onSave() {
    if (drawer.kind === 'closed') return;
    if (validateTeamForm(form) !== null) return;
    setSaving(true);
    setSaveError(null);
    try {
      if (drawer.kind === 'create') {
        await c.createTeam(formToCreateTeamReq(form));
      } else {
        await c.updateTeam(drawer.team.id, formToUpdateTeamReq(form));
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
    const target = del.team;
    setDel({ kind: 'deleting' });
    try {
      await c.deleteTeam(target.id);
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
    <section aria-labelledby="expert-teams-title" className="pane">
      <header>
        <h1 id="expert-teams-title">{t('pane.expert_teams.title')}</h1>
        <p className="muted">{t('pane.expert_teams.subtitle')}</p>
      </header>

      <div className="toolbar" role="search" aria-label="teams filters">
        <input
          type="search"
          value={nameFilter}
          placeholder={t('pane.expert_teams.filter_name_placeholder')}
          onChange={(e) => setNameFilter(e.target.value)}
          aria-label={t('pane.expert_teams.filter_name_placeholder')}
        />
        <RequireScope name="teams.write">
          <button type="button" onClick={openCreate}>
            {t('pane.expert_teams.btn_new')}
          </button>
        </RequireScope>
      </div>

      {load.kind === 'loading' && (
        <p role="status">{t('pane.expert_teams.loading')}</p>
      )}
      {load.kind === 'unavailable' && (
        <p role="alert" className="alert">
          {t('pane.expert_teams.unavailable')}
        </p>
      )}
      {load.kind === 'error' && (
        <p role="alert" className="alert">
          {t('common.failed', { message: load.message })}
        </p>
      )}
      {load.kind === 'ok' && filtered.length === 0 && (
        <p role="status" className="muted">
          {t('pane.expert_teams.empty')}
        </p>
      )}
      {load.kind === 'ok' && filtered.length > 0 && (
        <table aria-label="expert teams">
          <thead>
            <tr>
              <th>{t('pane.expert_teams.col_name')}</th>
              <th>{t('pane.expert_teams.col_lead')}</th>
              <th>{t('pane.expert_teams.col_members')}</th>
              <th>{t('pane.expert_teams.col_packs')}</th>
              <th>{t('pane.expert_teams.col_created')}</th>
              <th>{t('pane.expert_teams.col_actions')}</th>
            </tr>
          </thead>
          <tbody>
            {filtered.map((team) => (
              <tr key={team.id}>
                <td>{team.name}</td>
                <td>{resolvePersonaName(personas, team.lead_persona_id)}</td>
                <td>{team.member_persona_ids.length}</td>
                <td>
                  {team.recommended_pack_slugs.length === 0
                    ? '—'
                    : team.recommended_pack_slugs.map((slug) => (
                        <span key={slug} className="kind-tag">
                          {slug}
                        </span>
                      ))}
                </td>
                <td>{fmtDate(team.created_at)}</td>
                <td>
                  <RequireScope name="teams.write">
                    <button
                      type="button"
                      onClick={() => openEdit(team)}
                      aria-label={`edit ${team.name}`}
                    >
                      {t('pane.expert_teams.btn_edit')}
                    </button>
                  </RequireScope>{' '}
                  <RequireScope name="teams.delete">
                    <button
                      type="button"
                      onClick={() => setDel({ kind: 'confirming', team })}
                      aria-label={`delete ${team.name}`}
                    >
                      {t('pane.expert_teams.btn_delete')}
                    </button>
                  </RequireScope>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {drawer.kind !== 'closed' && (
        <div className="drawer-backdrop" role="dialog" aria-modal="true">
          <div className="drawer">
            <div className="drawer-header">
              <h2>
                {drawer.kind === 'create'
                  ? t('pane.expert_teams.drawer_create_title')
                  : t('pane.expert_teams.drawer_edit_title')}
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
                <span>{t('pane.expert_teams.field_name')}</span>
                <input
                  type="text"
                  value={form.name}
                  placeholder={t('pane.expert_teams.placeholder_name')}
                  onChange={(e) => setForm({ ...form, name: e.target.value })}
                  required
                />
              </label>
              <label>
                <span>{t('pane.expert_teams.field_description')}</span>
                <textarea
                  rows={3}
                  value={form.description}
                  onChange={(e) =>
                    setForm({ ...form, description: e.target.value })
                  }
                />
              </label>
              <fieldset>
                <legend>{t('pane.expert_teams.field_members')}</legend>
                {activePersonas.length === 0 && (
                  <p className="muted">
                    {t('pane.expert_teams.members_none_available')}
                  </p>
                )}
                {activePersonas.map((p) => {
                  const role = inferRoleTag(p);
                  return (
                    <label key={p.id} className="checkbox-row">
                      <input
                        type="checkbox"
                        checked={form.member_persona_ids.includes(p.id)}
                        onChange={() => onToggleMember(p.id)}
                        aria-label={`member ${p.name}`}
                      />
                      <span>{p.name}</span>{' '}
                      <span className={roleClassName(role)}>{role ?? '—'}</span>
                    </label>
                  );
                })}
              </fieldset>
              <label>
                <span>{t('pane.expert_teams.field_lead')}</span>
                <select
                  value={
                    form.member_persona_ids.includes(form.lead_persona_id)
                      ? form.lead_persona_id
                      : ''
                  }
                  onChange={(e) =>
                    setForm({ ...form, lead_persona_id: e.target.value })
                  }
                  aria-label={t('pane.expert_teams.field_lead')}
                >
                  <option value="">
                    {t('pane.expert_teams.lead_placeholder')}
                  </option>
                  {form.member_persona_ids.map((id) => (
                    <option key={id} value={id}>
                      {resolvePersonaName(personas, id)}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                <span>{t('pane.expert_teams.field_packs')}</span>
                <input
                  type="text"
                  value={form.pack_slugs_csv}
                  placeholder={t('pane.expert_teams.placeholder_packs')}
                  onChange={(e) =>
                    setForm({ ...form, pack_slugs_csv: e.target.value })
                  }
                />
              </label>
              <label>
                <span>{t('pane.expert_teams.field_glossary')}</span>
                <textarea
                  rows={8}
                  value={form.glossary_md}
                  placeholder={t('pane.expert_teams.placeholder_glossary')}
                  aria-label={t('pane.expert_teams.field_glossary')}
                  onChange={(e) =>
                    setForm({ ...form, glossary_md: e.target.value })
                  }
                />
              </label>
              {glossaryByteLength(form.glossary_md) > MAX_GLOSSARY_BYTES ? (
                <p role="status" className="alert">
                  {t('pane.expert_teams.glossary_over_cap', {
                    bytes: glossaryByteLength(form.glossary_md),
                    max: MAX_GLOSSARY_BYTES,
                  })}
                </p>
              ) : (
                <p className="muted">
                  {t('pane.expert_teams.glossary_cap_hint')}
                </p>
              )}
              {problem === 'no_members' && (
                <p role="status" className="muted">
                  {t('pane.expert_teams.hint_no_members')}
                </p>
              )}
              {problem === 'lead_not_member' && (
                <p role="status" className="muted">
                  {t('pane.expert_teams.hint_lead_not_member')}
                </p>
              )}
              {saveError && (
                <p role="alert" className="alert">
                  {saveError}
                </p>
              )}
              <div className="drawer-actions">
                <button type="button" onClick={closeDrawer}>
                  {t('pane.expert_teams.btn_cancel')}
                </button>
                <button type="submit" disabled={saving || problem !== null}>
                  {t('pane.expert_teams.btn_save')}
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
              <h2>{t('pane.expert_teams.delete_confirm_title')}</h2>
            </div>
            <p>{t('pane.expert_teams.delete_confirm_body')}</p>
            <p>
              <strong>{del.team.name}</strong>
            </p>
            <div className="drawer-actions">
              <button type="button" onClick={() => setDel({ kind: 'idle' })}>
                {t('pane.expert_teams.btn_cancel')}
              </button>
              <button
                type="button"
                onClick={() => {
                  void onConfirmDelete();
                }}
              >
                {t('pane.expert_teams.btn_delete')}
              </button>
            </div>
          </div>
        </div>
      )}
    </section>
  );
}
