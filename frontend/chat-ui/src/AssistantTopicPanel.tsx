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
import { useCallback, useEffect, useState } from 'react';
import type { Persona, Team } from '@xiaoguai/shared';
import { client } from './client';
import { SessionList } from './SessionList';
import { useI18n } from './i18n/I18nProvider';

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
  const { t } = useI18n();
  const [tab, setTab] = useState<Tab>('topics');
  const [personas, setPersonas] = useState<Persona[]>([]);
  const [teams, setTeams] = useState<Team[]>([]);
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
        const [ps, ts] = await Promise.all([client.listPersonas(), client.listTeams()]);
        if (!alive) return;
        setPersonas(ps.filter((p) => !p.archived));
        setTeams(ts.filter((tm) => !tm.archived));
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

  /** Attach to the active session, or lift to the parent for a new chat. Always
   *  best-effort on the attach; an error is surfaced inline (loadError reuse). */
  async function selectPersona(personaId: string) {
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
              active={isGeneralActive}
              onClick={() => void selectGeneral()}
            />
            {filteredPersonas.map((p) => (
              <AssistantRow
                key={p.id}
                label={p.name}
                active={isActivePersona(p.id)}
                onClick={() => void selectPersona(p.id)}
              />
            ))}

            {filteredTeams.length > 0 && (
              <>
                <div className="assistant-group-label">{t.ui.assistant.group_teams}</div>
                {filteredTeams.map((tm) => (
                  <AssistantRow
                    key={tm.id}
                    label={tm.name}
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

/** A single selectable assistant/team row with an active-state checkmark. */
function AssistantRow({
  label,
  active,
  onClick,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={`assistant-row${active ? ' active' : ''}`}
      onClick={onClick}
      aria-pressed={active}
      title={label}
    >
      <span className="assistant-row__name">{label}</span>
      {active && (
        <span className="assistant-row__check" aria-hidden="true">
          ✓
        </span>
      )}
    </button>
  );
}
