/**
 * AssistantTopicPanel — the ~270px list panel of the Cherry-Studio-style shell
 * (Phase 2 IA). Two tabs:
 *
 *   助手 (Assistants) — personas + teams (non-archived), with a pinned
 *     「通用」 row that detaches any persona. Selecting one attaches it to the
 *     active session (or, with no active session, lifts a "pending assistant"
 *     to the parent so the next-created session can attach it), then flips to
 *     the Topics tab.
 *   话题 (Topics) — the session history (the existing `SessionList`).
 *
 * Persona/team fetches are best-effort: a failure shows an inline error and
 * never crashes the shell.
 */
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import type { ExpertReadiness, Persona, Team } from '@xiaoguai/shared';
import { client } from './client';
import { SessionList } from './SessionList';
import { useBrandName } from './branding';
import { useI18n } from './i18n/I18nProvider';
import { interpolate } from './i18n';

/** Max characters of a persona's system prompt to show as its role line. */
const ROLE_DESC_MAX = 80;

/**
 * Derive a one-line role/description from a persona's system prompt: collapse
 * all whitespace (so multi-line prompts read as a single line) and truncate to
 * `ROLE_DESC_MAX` chars with an ellipsis. CSS clamps the rendered line further;
 * the truncation here just bounds the DOM string. Empty/blank → `null` so the
 * row simply omits the description rather than showing an empty line.
 */
function roleDescFromPrompt(systemPrompt: string): string | null {
  const collapsed = systemPrompt.replace(/\s+/g, ' ').trim();
  if (!collapsed) return null;
  return collapsed.length > ROLE_DESC_MAX
    ? `${collapsed.slice(0, ROLE_DESC_MAX)}…`
    : collapsed;
}

/** The assistant pending attachment for the NEXT-created session (no active
 *  session yet). `null` = explicit 通用 (no persona). */
export type PendingAssistant =
  | { kind: 'persona'; id: string }
  | { kind: 'team'; id: string }
  | { kind: 'general' };

interface StoredSession {
  id: string;
  title: string;
  working_dir?: string;
}

interface Props {
  /** Topic-tab session list (passed straight through to `SessionList`). */
  sessions: StoredSession[];
  onRename?: (id: string, title: string) => void;
  onDelete?: (id: string) => void;
  todayTokens?: number | null;
  tokensLoading?: boolean;
  activeWorkingDir?: string;
  onSaveWorkingDir?: (sessionId: string, workingDir: string) => Promise<void>;
  /** The active session id (when viewing one), else undefined. Drives whether
   *  an assistant selection attaches now or is held as "pending". */
  activeSessionId?: string;
  /** Lift the selection for a NEW chat (no active session). The parent holds it
   *  and attaches it once the next session is created. */
  pendingAssistant: PendingAssistant | null;
  onSelectAssistant: (next: PendingAssistant | null) => void;
}

type Tab = 'topics' | 'assistants';

export function AssistantTopicPanel({
  sessions,
  onRename,
  onDelete,
  todayTokens,
  tokensLoading,
  activeWorkingDir,
  onSaveWorkingDir,
  activeSessionId,
  pendingAssistant,
  onSelectAssistant,
}: Props) {
  const { t, locale } = useI18n();
  const isZh = locale === 'zh-CN';
  const navigate = useNavigate();
  // White-label wordmark shown above the list tabs, falling back to the locale
  // default assistant name when no owner branding is set.
  const brandName = useBrandName() || t.ui.assistant_name;
  const [tab, setTab] = useState<Tab>('topics');
  const [personas, setPersonas] = useState<Persona[]>([]);
  const [teams, setTeams] = useState<Team[]>([]);
  // v1.34 — expert prerequisites keyed by persona name; a not-ready expert's
  // row is locked (can't be selected) until its required skills are installed.
  const [experts, setExperts] = useState<ExpertReadiness[]>([]);
  const [loadError, setLoadError] = useState(false);
  const [query, setQuery] = useState('');
  // The persona/team currently attached to the active session (reflected with a
  // checkmark). For a sessionless draft, the parent's `pendingAssistant` drives
  // the highlight instead.
  const [activePersonaId, setActivePersonaId] = useState<string | null>(null);

  // Fetch the assistant catalogue once on mount. Best-effort: a failure flips
  // an inline error flag rather than throwing out of the shell.
  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        // Experts (prerequisites/readiness) are best-effort and MUST NOT gate
        // the panel: if the endpoint fails, every persona stays selectable.
        const [ps, ts, ex] = await Promise.all([
          client.listPersonas(),
          client.listTeams(),
          client.listExperts().catch(() => null),
        ]);
        if (!alive) return;
        setPersonas(ps.filter((p) => !p.archived));
        setTeams(ts.filter((tm) => !tm.archived));
        if (ex) setExperts(ex.experts);
        setLoadError(false);
      } catch {
        if (alive) setLoadError(true);
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  // Reflect the active session's attached persona (best-effort). Re-runs on
  // session switch; a sessionless draft clears the highlight.
  useEffect(() => {
    let alive = true;
    if (!activeSessionId) {
      setActivePersonaId(null);
      return;
    }
    void (async () => {
      try {
        const p = await client.getSessionPersona(activeSessionId);
        if (alive) setActivePersonaId(p?.id ?? null);
      } catch {
        if (alive) setActivePersonaId(null);
      }
    })();
    return () => {
      alive = false;
    };
  }, [activeSessionId]);

  // Whether a given persona/team is the current selection (for the checkmark).
  const isActivePersona = useCallback(
    (id: string): boolean => {
      if (activeSessionId) return activePersonaId === id;
      return pendingAssistant?.kind === 'persona' && pendingAssistant.id === id;
    },
    [activeSessionId, activePersonaId, pendingAssistant],
  );
  const isActiveTeam = useCallback(
    (id: string): boolean =>
      !activeSessionId && pendingAssistant?.kind === 'team' && pendingAssistant.id === id,
    [activeSessionId, pendingAssistant],
  );
  const isGeneralActive =
    activeSessionId != null
      ? activePersonaId === null
      : pendingAssistant?.kind === 'general' || pendingAssistant == null;

  // v1.34 — expert blueprint by persona name (empty until /v1/experts loads).
  const expertByName = useMemo(() => {
    const m = new Map<string, ExpertReadiness>();
    for (const e of experts) m.set(e.persona_name, e);
    return m;
  }, [experts]);

  /**
   * Lock info for a persona whose expert blueprint isn't ready yet, else null.
   * `hint` names the unmet required groups so the operator knows what to
   * install; a persona with no blueprint (an ordinary persona) is never locked.
   */
  const lockFor = useCallback(
    (personaName: string): { hint: string } | null => {
      const e = expertByName.get(personaName);
      if (!e || e.ready) return null;
      const items = e.required
        .filter((g) => !g.satisfied)
        .map((g) => (isZh ? (g.label_zh ?? g.label) : g.label))
        .join(isZh ? '、' : ', ');
      return { hint: interpolate(t.ui.assistant.locked_hint, { items }) };
    },
    [expertByName, isZh, t],
  );

  /** Attach to the active session, or lift to the parent for a new chat. Always
   *  best-effort on the attach; an error is surfaced inline (loadError reuse).
   *  A locked (not-ready) expert can't be attached — the row disables click,
   *  and this guards it defensively. */
  async function selectPersona(personaId: string) {
    const p = personas.find((x) => x.id === personaId);
    if (p && lockFor(p.name)) return;
    if (activeSessionId) {
      try {
        await client.attachSessionPersona(activeSessionId, personaId);
        setActivePersonaId(personaId);
      } catch {
        setLoadError(true);
        return;
      }
    } else {
      onSelectAssistant({ kind: 'persona', id: personaId });
    }
    setTab('topics');
  }

  async function selectTeam(teamId: string) {
    if (activeSessionId) {
      try {
        await client.attachSessionTeam(activeSessionId, teamId);
        // Attaching a team also attaches its lead persona server-side; refresh
        // the reflected persona so the highlight is accurate.
        try {
          const p = await client.getSessionPersona(activeSessionId);
          setActivePersonaId(p?.id ?? null);
        } catch {
          /* best effort — keep prior highlight */
        }
      } catch {
        setLoadError(true);
        return;
      }
    } else {
      onSelectAssistant({ kind: 'team', id: teamId });
    }
    setTab('topics');
  }

  async function selectGeneral() {
    if (activeSessionId) {
      try {
        await client.detachSessionPersona(activeSessionId);
        setActivePersonaId(null);
      } catch {
        setLoadError(true);
        return;
      }
    } else {
      onSelectAssistant({ kind: 'general' });
    }
    setTab('topics');
  }

  const q = query.trim().toLowerCase();
  const filteredPersonas = q
    ? personas.filter((p) => p.name.toLowerCase().includes(q))
    : personas;
  const filteredTeams = q
    ? teams.filter((tm) => tm.name.toLowerCase().includes(q))
    : teams;

  return (
    <aside className="list-panel">
      <div className="list-brand" title={brandName}>
        {brandName}
      </div>
      <div className="list-tabs" role="tablist" aria-label={t.ui.assistant.tab_topics}>
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'assistants'}
          className={`list-tab${tab === 'assistants' ? ' active' : ''}`}
          onClick={() => setTab('assistants')}
        >
          {t.ui.assistant.tab_assistants}
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'topics'}
          className={`list-tab${tab === 'topics' ? ' active' : ''}`}
          onClick={() => setTab('topics')}
        >
          {t.ui.assistant.tab_topics}
        </button>
      </div>

      {tab === 'topics' ? (
        <SessionList
          sessions={sessions}
          onRename={onRename}
          onDelete={onDelete}
          todayTokens={todayTokens}
          tokensLoading={tokensLoading}
          activeWorkingDir={activeWorkingDir}
          onSaveWorkingDir={onSaveWorkingDir}
        />
      ) : (
        <div className="assistant-pane">
          <input
            type="text"
            className="assistant-search"
            value={query}
            placeholder={t.ui.assistant.search_placeholder}
            spellCheck={false}
            autoComplete="off"
            onChange={(e) => setQuery(e.target.value)}
            aria-label={t.ui.assistant.search_placeholder}
          />

          {loadError && (
            <p className="assistant-error" role="alert">
              {t.ui.assistant.error_load}
            </p>
          )}

          <div className="assistant-groups">
            <div className="assistant-group-label">{t.ui.assistant.group_personas}</div>
            <AssistantRow
              label={t.ui.assistant.general}
              desc={t.ui.assistant.general_desc}
              active={isGeneralActive}
              onClick={() => void selectGeneral()}
            />
            {filteredPersonas.map((p) => {
              const lock = lockFor(p.name);
              return (
                <AssistantRow
                  key={p.id}
                  label={p.name}
                  desc={roleDescFromPrompt(p.system_prompt)}
                  active={isActivePersona(p.id)}
                  lockHint={lock?.hint ?? null}
                  lockTitle={t.ui.assistant.locked_title}
                  ctaLabel={t.ui.assistant.locked_cta}
                  onInstall={() => navigate('/skills')}
                  onClick={() => void selectPersona(p.id)}
                />
              );
            })}

            {filteredTeams.length > 0 && (
              <>
                <div className="assistant-group-label">{t.ui.assistant.group_teams}</div>
                {filteredTeams.map((tm) => (
                  <AssistantRow
                    key={tm.id}
                    label={tm.name}
                    desc={tm.description?.trim() || null}
                    active={isActiveTeam(tm.id)}
                    onClick={() => void selectTeam(tm.id)}
                  />
                ))}
              </>
            )}

            {!loadError && filteredPersonas.length === 0 && filteredTeams.length === 0 && (
              <p className="assistant-empty">{t.ui.assistant.empty}</p>
            )}
          </div>
        </div>
      )}
    </aside>
  );
}

/**
 * A single selectable assistant/team row: the name, an optional muted role /
 * purpose line beneath it, and an active-state checkmark. `desc` is `null` when
 * the assistant has no role text to show (the line is then omitted entirely).
 *
 * v1.34 — when `lockHint` is set the row is a NOT-READY expert: it can't be
 * selected (the main click is inert + `aria-disabled`), shows the unmet
 * prerequisites, and offers a compact CTA that routes to the Skills page.
 */
function AssistantRow({
  label,
  desc,
  active,
  onClick,
  lockHint = null,
  lockTitle,
  ctaLabel,
  onInstall,
}: {
  label: string;
  desc?: string | null;
  active: boolean;
  onClick: () => void;
  lockHint?: string | null;
  lockTitle?: string;
  ctaLabel?: string;
  onInstall?: () => void;
}) {
  const locked = lockHint != null;
  return (
    <div
      className={`assistant-row${active ? ' active' : ''}${locked ? ' locked' : ''}`}
    >
      <button
        type="button"
        className="assistant-row__main"
        onClick={locked ? undefined : onClick}
        aria-pressed={active}
        aria-disabled={locked}
        title={locked ? lockTitle : desc ? `${label} — ${desc}` : label}
      >
        <span className="assistant-row__text">
          <span className="assistant-row__name">
            {locked && (
              <span className="assistant-row__lock" aria-hidden="true">
                🔒{' '}
              </span>
            )}
            {label}
          </span>
          {locked ? (
            <span className="assistant-row__desc assistant-row__lockhint">{lockHint}</span>
          ) : (
            desc && <span className="assistant-row__desc">{desc}</span>
          )}
        </span>
        {active && !locked && (
          <span className="assistant-row__check" aria-hidden="true">
            ✓
          </span>
        )}
      </button>
      {locked && ctaLabel && onInstall && (
        <button type="button" className="assistant-row__cta" onClick={onInstall}>
          {ctaLabel}
        </button>
      )}
    </div>
  );
}
