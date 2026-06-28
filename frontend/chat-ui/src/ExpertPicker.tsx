/**
 * ExpertPicker — T3.5 (chat-ui expert picker).
 *
 * Compact chip in the chat header showing the expert (persona or team)
 * attached to the ACTIVE session, or a neutral "Expert" label when none.
 * Clicking opens a popover with:
 *   - a "一句话找专家" goal input that calls `suggestExperts` and renders
 *     ranked results (click = attach, no extra dialog),
 *   - a text filter over two groups (personas / teams).
 *
 * Selecting a persona calls `attachSessionPersona`; selecting a team calls
 * `attachSessionTeam` (the backend also attaches the team's lead persona).
 * Remove detaches both team and persona.
 *
 * Availability: a 503 from any expert endpoint means the personas subsystem
 * is not wired — the picker renders nothing. All other failures surface as
 * inline error text inside the popover; they never crash the chat.
 */

import { useEffect, useRef, useState } from 'react';
import type { ExpertSuggestion, Persona, Team } from '@xiaoguai/shared';
import { client } from './client';
import { useI18n } from './i18n/I18nProvider';
import { interpolate } from './i18n';
import {
  filterByQuery,
  formatScore,
  isExpertsUnavailable,
  isNotAttached,
  readableMemberName,
  selectablePersonas,
  selectableTeams,
  sortSuggestions,
  teamForPackSlug,
} from './expertPickerHelpers';
import type { ActiveExpert } from './expertPickerHelpers';

interface ExpertPickerProps {
  sessionId: string | undefined;
  /**
   * T5.2 — fired whenever the active expert changes (load / attach /
   * remove / session switch). Lets ChatPage know whether a team is
   * attached without re-fetching `getSessionTeam` itself.
   */
  onActiveChange?: (active: ActiveExpert | null) => void;
  /**
   * Phase 4c — a pack slug carried from the Skills "Use in chat" deep-link
   * (chat-ui route `?team=<slug>`). On arrival the picker opens, resolves the
   * slug to the pack's activated team, and — when a session exists — attaches
   * it (otherwise shows a "send a message first" hint). Kept as a prop (not a
   * router hook) so ExpertPicker stays router-free and unit-testable.
   */
  deepLinkTeamSlug?: string | null;
  /** Phase 4c — fired once the deep-link has been consumed, so the parent can
   *  clear the `?team=` param and not re-trigger on every render. */
  onDeepLinkConsumed?: () => void;
}

export function ExpertPicker({
  sessionId,
  onActiveChange,
  deepLinkTeamSlug,
  onDeepLinkConsumed,
}: ExpertPickerProps) {
  const { t } = useI18n();
  const [active, setActiveState] = useState<ActiveExpert | null>(null);

  /** Single setter so the parent notification can never be forgotten. */
  function setActive(next: ActiveExpert | null) {
    setActiveState(next);
    onActiveChange?.(next);
  }
  const [unavailable, setUnavailable] = useState(false);
  const [open, setOpen] = useState(false);
  /** `null` = catalog not loaded yet (lazy-loaded on first open). */
  const [personas, setPersonas] = useState<Persona[] | null>(null);
  const [teams, setTeams] = useState<Team[] | null>(null);
  const [filter, setFilter] = useState('');
  const [goal, setGoal] = useState('');
  /** `null` = no suggest run yet; `[]` = ran and nothing matched. */
  const [suggestions, setSuggestions] = useState<ExpertSuggestion[] | null>(null);
  const [suggesting, setSuggesting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /**
   * Phase 4c — transient note shown inside the popover after a Skills
   * deep-link: either "team attached" or "send a message first to attach". */
  const [deepLinkHint, setDeepLinkHint] = useState<string | null>(null);
  const popoverRef = useRef<HTMLDivElement>(null);

  // On session switch: reset panel state and load the active expert.
  // Team takes display precedence over persona (design §2.3).
  useEffect(() => {
    setActive(null);
    setOpen(false);
    setError(null);
    setSuggestions(null);
    setGoal('');
    setFilter('');
    setDeepLinkHint(null);
    if (!sessionId) return;
    let cancelled = false;
    void (async () => {
      try {
        const team = await client.getSessionTeam(sessionId);
        if (cancelled) return;
        if (team) {
          setActive({ kind: 'team', id: team.id, name: team.name });
          return;
        }
        const persona = await client.getSessionPersona(sessionId);
        if (cancelled) return;
        if (persona) {
          setActive({ kind: 'persona', id: persona.id, name: persona.name });
        }
      } catch (err) {
        if (cancelled) return;
        if (isExpertsUnavailable(err)) {
          setUnavailable(true);
          return;
        }
        setError(
          interpolate(t.ui.expert.error_load, { message: (err as Error).message }),
        );
      }
    })();
    return () => {
      cancelled = true;
    };
    // `t` only swaps the error language; reloading on locale change is noise.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  // Close the popover when clicking outside it.
  useEffect(() => {
    if (!open) return;
    function handleOutside(e: MouseEvent) {
      if (popoverRef.current && !popoverRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    }
    document.addEventListener('mousedown', handleOutside);
    return () => document.removeEventListener('mousedown', handleOutside);
  }, [open]);

  /**
   * Lazy-load the persona/team catalog (once). Returns the freshly-selectable
   * teams so a caller that needs them immediately (the deep-link) doesn't race
   * the `teams` state update. Idempotent: a second call is a no-op + re-returns
   * whatever is in state. Surfaces 503 as "unavailable", other errors inline.
   */
  async function loadCatalog(): Promise<Team[] | null> {
    if (personas !== null) return teams;
    try {
      const [ps, ts] = await Promise.all([client.listPersonas(), client.listTeams()]);
      const selTeams = selectableTeams(ts);
      setPersonas(selectablePersonas(ps));
      setTeams(selTeams);
      return selTeams;
    } catch (err) {
      if (isExpertsUnavailable(err)) {
        setUnavailable(true);
        return null;
      }
      setError(
        interpolate(t.ui.expert.error_load, { message: (err as Error).message }),
      );
      return null;
    }
  }

  // Phase 4c — Skills deep-link ("Use in chat"): open the picker, resolve the
  // pack slug to its activated team, and attach it when a session exists
  // (otherwise prompt the operator to start one). Consumed exactly once so the
  // parent can drop the `?team=` param without re-firing.
  useEffect(() => {
    const slug = deepLinkTeamSlug?.trim();
    if (!slug || unavailable) return;
    let cancelled = false;
    void (async () => {
      setOpen(true);
      setError(null);
      const loaded = await loadCatalog();
      if (cancelled) return;
      const team = loaded ? teamForPackSlug(loaded, slug) : undefined;
      if (team) {
        if (sessionId) {
          await attach('team', team.id, team.name);
          if (cancelled) return;
          setDeepLinkHint(interpolate(t.ui.expert.deeplink_attached, { team: team.name }));
        } else {
          // No session to attach to yet — pre-filter so the team is visible
          // and tell the operator how to finish the flow.
          setFilter(team.name);
          setDeepLinkHint(
            interpolate(t.ui.expert.deeplink_need_session, { team: team.name }),
          );
        }
      }
      onDeepLinkConsumed?.();
    })();
    return () => {
      cancelled = true;
    };
    // Only re-run when the deep-link slug or the session identity changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [deepLinkTeamSlug, sessionId]);

  if (!sessionId || unavailable) {
    // Even with no session we must signal "consumed" so the parent clears the
    // deep-link param; the team can't be attached until a session exists.
    return null;
  }

  /** Toggle the popover; lazy-load the persona/team catalog on first open. */
  async function togglePanel() {
    const opening = !open;
    setOpen(opening);
    if (!opening) return;
    await loadCatalog();
  }

  /** Attach a persona or team to the session; team also attaches its lead. */
  async function attach(kind: 'persona' | 'team', id: string, name: string) {
    if (!sessionId || busy) return;
    setBusy(true);
    setError(null);
    try {
      if (kind === 'team') {
        await client.attachSessionTeam(sessionId, id);
      } else {
        await client.attachSessionPersona(sessionId, id);
      }
      setActive({ kind, id, name });
      setSuggestions(null);
      setGoal('');
      setOpen(false);
    } catch (err) {
      if (isExpertsUnavailable(err)) {
        setUnavailable(true);
        return;
      }
      setError(
        interpolate(t.ui.expert.error_attach, { message: (err as Error).message }),
      );
    } finally {
      setBusy(false);
    }
  }

  /** Remove the expert entirely: detach team, then persona (404 = fine). */
  async function removeExpert() {
    if (!sessionId || busy) return;
    setBusy(true);
    setError(null);
    try {
      try {
        await client.detachSessionTeam(sessionId);
      } catch (err) {
        if (!isNotAttached(err)) throw err;
      }
      try {
        await client.detachSessionPersona(sessionId);
      } catch (err) {
        if (!isNotAttached(err)) throw err;
      }
      setActive(null);
    } catch (err) {
      if (isExpertsUnavailable(err)) {
        setUnavailable(true);
        return;
      }
      setError(
        interpolate(t.ui.expert.error_detach, { message: (err as Error).message }),
      );
    } finally {
      setBusy(false);
    }
  }

  /** "一句话找专家": rank personas + teams against a free-text goal. */
  async function runSuggest() {
    const trimmed = goal.trim();
    if (!trimmed || suggesting) return;
    setSuggesting(true);
    setError(null);
    try {
      const res = await client.suggestExperts(trimmed);
      setSuggestions(sortSuggestions(res.suggestions));
    } catch (err) {
      if (isExpertsUnavailable(err)) {
        setUnavailable(true);
        return;
      }
      setSuggestions(null);
      setError(
        interpolate(t.ui.expert.error_suggest, { message: (err as Error).message }),
      );
    } finally {
      setSuggesting(false);
    }
  }

  const chipLabel = active ? active.name : t.ui.expert.pick_label;
  const visiblePersonas = filterByQuery(personas ?? [], filter);
  const visibleTeams = filterByQuery(teams ?? [], filter);

  return (
    <div className="expert-picker" ref={popoverRef}>
      <button
        type="button"
        className={`expert-chip${active ? ' expert-chip--active' : ''}`}
        onClick={() => void togglePanel()}
        aria-expanded={open}
        aria-haspopup="true"
        title={t.ui.expert.chip_title}
        data-testid="expert-chip"
      >
        {chipLabel}
      </button>

      {open && (
        <div
          className="expert-popover"
          role="dialog"
          aria-label={t.ui.expert.panel_title}
          data-testid="expert-popover"
        >
          <div className="expert-popover__header">
            <span>{t.ui.expert.panel_title}</span>
            {active && (
              <button
                type="button"
                className="expert-popover__remove"
                onClick={() => void removeExpert()}
                disabled={busy}
                data-testid="expert-remove"
              >
                {t.ui.expert.remove}
              </button>
            )}
          </div>

          {error && (
            <div className="expert-popover__error" role="alert" data-testid="expert-error">
              {error}
            </div>
          )}

          {/* Phase 4c — Skills deep-link outcome (attached / needs a session). */}
          {deepLinkHint && (
            <div
              className="expert-popover__deeplink"
              role="status"
              data-testid="expert-deeplink-hint"
            >
              {deepLinkHint}
            </div>
          )}

          {/* Feature ⑭ — Team Run discoverability: when a team is the active
              expert, point the operator at the Team Run entry by the composer
              and seed an example goal. The orchestrate run itself lives in the
              composer (always execute mode — hence the consult caveat); this is
              a read-only "here's how to run the team" affordance. */}
          {active?.kind === 'team' && (
            <div className="expert-popover__teamrun" data-testid="expert-teamrun-hint">
              <span className="expert-popover__teamrun-lead">{t.ui.teamrun.button}</span>
              <span className="expert-popover__teamrun-body">
                {t.ui.expert.teamrun_discover}
              </span>
              <span className="expert-popover__teamrun-example" title={t.ui.teamrun.example_goal}>
                {t.ui.teamrun.example_goal}
              </span>
              <span className="expert-popover__teamrun-consult">
                {t.ui.teamrun.disabled_consult}
              </span>
            </div>
          )}

          {/* "一句话找专家" — describe a goal, get ranked experts. */}
          <div className="expert-popover__suggest">
            <label className="expert-popover__suggest-label" htmlFor="expert-goal-input">
              {t.ui.expert.suggest_label}
            </label>
            <div className="expert-popover__suggest-row">
              <input
                id="expert-goal-input"
                className="expert-popover__input"
                type="text"
                value={goal}
                placeholder={t.ui.expert.suggest_placeholder}
                onChange={(e) => setGoal(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') {
                    e.preventDefault();
                    void runSuggest();
                  }
                }}
                data-testid="expert-goal-input"
              />
              <button
                type="button"
                className="expert-popover__suggest-btn"
                onClick={() => void runSuggest()}
                disabled={suggesting || !goal.trim()}
                data-testid="expert-suggest-btn"
              >
                {suggesting ? t.ui.expert.suggest_searching : t.ui.expert.suggest_button}
              </button>
            </div>
            {suggestions !== null && suggestions.length === 0 && (
              <div className="expert-popover__hint" data-testid="expert-no-match">
                {t.ui.expert.suggest_empty}
              </div>
            )}
            {suggestions !== null && suggestions.length > 0 && (
              <div className="expert-popover__suggestions">
                {suggestions.map((s) => (
                  <button
                    key={`${s.kind}:${s.id}`}
                    type="button"
                    className="expert-popover__row"
                    onClick={() => void attach(s.kind, s.id, s.name)}
                    disabled={busy}
                    data-testid="expert-suggestion"
                  >
                    <span className="expert-popover__name">{s.name}</span>
                    <span className={`expert-popover__kind expert-popover__kind--${s.kind}`}>
                      {s.kind === 'team' ? t.ui.expert.kind_team : t.ui.expert.kind_persona}
                    </span>
                    <span className="expert-popover__score">{formatScore(s.score)}</span>
                  </button>
                ))}
              </div>
            )}
          </div>

          {/* Catalog: text filter over two groups (personas / teams). */}
          <input
            className="expert-popover__input expert-popover__filter"
            type="text"
            value={filter}
            placeholder={t.ui.expert.filter_placeholder}
            onChange={(e) => setFilter(e.target.value)}
            data-testid="expert-filter"
          />

          <div className="expert-popover__group-title">{t.ui.expert.group_personas}</div>
          <div className="expert-popover__list">
            {visiblePersonas.length === 0 ? (
              <div className="expert-popover__hint">{t.ui.expert.empty_group}</div>
            ) : (
              visiblePersonas.map((p) => (
                <button
                  key={p.id}
                  type="button"
                  className="expert-popover__row"
                  onClick={() => void attach('persona', p.id, p.name)}
                  disabled={busy}
                  data-testid="expert-persona-row"
                >
                  <span className="expert-popover__name">{p.name}</span>
                </button>
              ))
            )}
          </div>

          <div className="expert-popover__group-title">{t.ui.expert.group_teams}</div>
          {/* Phase 4c — first-run guidance: where teams come from + what a team
              run does. Shown above the team rows so it reads as a lead-in. */}
          <div className="expert-popover__firstrun" data-testid="expert-firstrun-hint">
            {t.ui.expert.firstrun_hint}
          </div>
          <div className="expert-popover__list">
            {visibleTeams.length === 0 ? (
              <div className="expert-popover__hint">{t.ui.expert.empty_group}</div>
            ) : (
              visibleTeams.map((team) => (
                <button
                  key={team.id}
                  type="button"
                  className="expert-popover__row expert-popover__row--team"
                  onClick={() => void attach('team', team.id, team.name)}
                  disabled={busy}
                  data-testid="expert-team-row"
                >
                  <span className="expert-popover__name">{team.name}</span>
                  {team.description ? (
                    <span className="expert-popover__desc">{team.description}</span>
                  ) : (
                    // No description → show the team's members readably so a
                    // pack-derived team (slug/agent ids) is still legible.
                    team.member_persona_ids.length > 0 && (
                      <span className="expert-popover__desc">
                        {team.member_persona_ids.map(readableMemberName).join(' · ')}
                      </span>
                    )
                  )}
                </button>
              ))
            )}
          </div>
        </div>
      )}
    </div>
  );
}
