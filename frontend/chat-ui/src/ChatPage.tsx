import { useEffect, useRef, useState } from 'react';
import { useNavigate, useParams, useSearchParams } from 'react-router-dom';
import type {
  AgentEvent,
  ContentBlock,
  HotlResolvedEvent,
  LlmProviderView,
  LoopResponse,
  Message,
  OrchestrateEvent,
  TurnMode,
} from '@xiaoguai/shared';
import { client } from './client';
import { CitationStrip } from './citations';
import { CopyButton } from './codeblock';
import { MarkdownBody } from './markdown';
import { HotlBanner } from './HotlBanner';
import type { HotlPendingState } from './HotlBanner';
import { SseReconnectBanner } from './SseReconnectBanner';
import { WatchIndicator } from './WatchIndicator';
import { teamForPackSlug } from './expertPickerHelpers';
import { MessageToolbar } from './MessageToolbar';
import { ChatHeaderBar } from './ChatHeaderBar';
import { ModeToggle, getStoredChatMode, setStoredChatMode } from './ModeToggle';
import { useI18n } from './i18n/I18nProvider';
import { interpolate } from './i18n';
import { useBrandName } from './branding';
import { isLoopLive, parseLoopCommand, shortLoopId } from './loopCommands';
import type { LoopCommand } from './loopCommands';

type CitationBlock = Extract<ContentBlock, { type: 'citation' }>;

interface Props {
  onSessionCreated: (s: { id: string; title: string }) => void;
  /** Called when an opened session no longer exists server-side (404 on history
   *  load) — e.g. a stale localStorage entry after a server/DB reset. The shell
   *  prunes it from the sidebar so it stops being clickable. */
  onSessionMissing?: (id: string) => void;
  /**
   * Phase 2 (Cherry-Studio IA) — called with the new session id right after
   * it's created, BEFORE the first turn runs, so the shell can attach the
   * assistant (persona/team) the operator pre-selected for a new chat. The
   * shell's implementation is best-effort and never rejects.
   */
  onSessionAttached?: (newSessionId: string) => Promise<void>;
}

interface DisplayBubble {
  /**
   * L2b — `system` bubbles are locally-generated /loop confirmations,
   * status listings and errors. They are never sent to the agent.
   */
  kind: 'user' | 'assistant' | 'tool' | 'system';
  text: string;
  toolName?: string;
  toolError?: boolean;
  /** When true, a streaming assistant turn is currently appending to this bubble. */
  streaming?: boolean;
  /** v0.9.3 — citation chips attached to an assistant turn. */
  citations?: CitationBlock[];
  /**
   * v1.1.2 — when this bubble came from a persisted assistant message,
   * its message id (so "Branch from here" knows the cutoff). Bubbles
   * produced live by streaming have no id yet and don't get the button.
   */
  messageId?: string;
  /**
   * L2b — when set, this system bubble is an un-armed `/loop` confirmation
   * showing Arm / Cancel actions. Cleared once the operator decides.
   */
  loopConfirm?: { prompt: string };
}

const DEV_USER_ID = 'usr_dev';
// Fallback model when no provider is configured or the providers list can't be
// fetched. The model picker (populated from the registered providers) normally
// supplies the real choice; empty → the server applies its own default model.
const DEFAULT_MODEL = '';

/**
 * L2b — defaults the chat-ui surfaces in the /loop confirmation bubble. They
 * mirror the backend's `CreateLoopParams` fallbacks; the create call itself
 * omits these fields and lets the server apply them, so a single source of
 * truth stays on the server.
 */
const LOOP_DEFAULT_INTERVAL_SECS = 300;
const LOOP_DEFAULT_MAX_TICKS = 50;
const LOOP_DEFAULT_TTL_HOURS = 24;

/** Grow a textarea to fit its content, up to a cap, then scroll. */
function autoGrow(ta: HTMLTextAreaElement | null) {
  if (!ta) return;
  ta.style.height = 'auto';
  ta.style.height = `${Math.min(ta.scrollHeight, 200)}px`;
}

export function ChatPage({ onSessionCreated, onSessionMissing, onSessionAttached }: Props) {
  const { t } = useI18n();
  // White-label assistant name (owner-set), falling back to the locale default.
  const brandName = useBrandName() || t.ui.assistant_name;
  const { id: routeId } = useParams<{ id: string }>();
  const navigate = useNavigate();
  // Phase 4c — Skills "Use in chat" deep-link carries the activated pack slug
  // as `?team=<slug>`; the deep-link effect below resolves it to the pack's
  // team and attaches it. We clear the param once consumed so a reload /
  // session switch doesn't re-trigger the attach.
  const [searchParams, setSearchParams] = useSearchParams();
  const deepLinkTeamSlug = searchParams.get('team');
  const [sessionId, setSessionId] = useState<string | undefined>(routeId);
  const [bubbles, setBubbles] = useState<DisplayBubble[]>([]);
  const [draft, setDraft] = useState('');
  const [streaming, setStreaming] = useState(false);
  // Model picker — aggregated from the registered providers' model lists, so
  // the operator can pick the model instead of it being hard-coded.
  const [models, setModels] = useState<string[]>([]);
  const [model, setModel] = useState<string>(() => {
    try {
      return localStorage.getItem('xiaoguai.chat.model') ?? '';
    } catch {
      return '';
    }
  });
  /**
   * T5.2 — consult/execute turn mode. Sticky per session via localStorage
   * (re-loaded on session switch); a sessionless draft starts in execute.
   */
  const [mode, setMode] = useState<TurnMode>(() =>
    routeId ? getStoredChatMode(routeId) : 'execute',
  );
  /**
   * T5.2 — id of the team attached to this session, or null. Derived on
   * session load/change from `getSessionTeam` (a team attached → its id; none
   * → null). Gates the "team parallel run" entry.
   */
  const [teamId, setTeamId] = useState<string | null>(null);
  /**
   * Read-only active-assistant display name for the header. The assistant is
   * picked in the list panel's 助手 tab; the header just reflects the attached
   * team (preferred) / persona, or the localized 通用 fallback when none.
   * Derived on session load/change alongside `teamId`.
   */
  const [assistantName, setAssistantName] = useState<string>(t.ui.assistant.general);
  /** T5.2 — true while an orchestrate run streams; blocks normal sends. */
  const [orchestrating, setOrchestrating] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  /** v1.3.x — non-null while an HotL escalation is pending for this session. */
  const [hotlPending, setHotlPending] = useState<HotlPendingState | null>(null);
  /**
   * Sprint-12 S12-8 — latest matching `hotl_resolved` SSE event. Reduced
   * by `applyEvent`; HotlBanner consumes it as the primary clear signal.
   * Reset on session change or when HotlBanner reports cleared.
   */
  const [hotlResolved, setHotlResolved] = useState<HotlResolvedEvent | null>(
    null,
  );
  /**
   * sprint-11 S11-2b — non-null while sendMessage is sleeping between
   * retries. Set by the onReconnect callback, cleared inside applyEvent on
   * the first event of the resumed stream.
   */
  const [reconnect, setReconnect] = useState<{ attempt: number; delayMs: number } | null>(
    null,
  );
  /**
   * Feature ⑥ — true when the opened session has a turn still running
   * server-side (`GET /v1/sessions/{id}/status` → `in_flight: true`) but THIS
   * tab is not locally streaming it. Surfaces a small non-blocking "task still
   * running" indicator. Cleared when a local turn starts/streams, when status
   * returns false, or on session change. Best-effort: a failed status fetch
   * never disrupts the chat.
   */
  const [remoteRunning, setRemoteRunning] = useState(false);
  /** Feature ⑥ — handle for the light ~5s poll that auto-clears the indicator. */
  const remotePollRef = useRef<ReturnType<typeof setInterval> | null>(null);
  /**
   * Feature ⑥ — mirrors `streaming` for use inside the poll's `setInterval`
   * closure (which would otherwise capture a stale value). The remote
   * indicator must never show while this tab is itself streaming a turn.
   */
  const streamingRef = useRef(false);
  const abortRef = useRef<(() => void) | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  /**
   * F5 — set while sendMessage is between a drop and the first event of the
   * resumed stream. The backend has no SSE resume (it re-runs the turn from
   * scratch), so on that first resumed event we roll the in-flight turn back
   * to the user's message; otherwise the re-generated text would append to
   * the partial bubble and duplicate. Amends DEC-LLD-CHAT-UI-003 (which
   * previously left the partial bubble untouched on reconnect).
   */
  const reconnectingRef = useRef(false);
  /**
   * Session id whose `/sessions/:id` navigation was issued by send() itself
   * (create-session-then-navigate). The route-change effect below must NOT
   * wipe live turn state for that navigation: the SSE stream is already
   * running, and under a delayed effect tick (observed on webkit) the reset
   * raced the first SSE events and erased an applied `hotl_pending` — the
   * HotlBanner then never mounted. A real user-driven session switch always
   * carries a routeId different from this marker and still resets below.
   */
  const selfNavigatedSessionRef = useRef<string | null>(null);

  /**
   * Feature ⑥ — stop any running status poll. Safe to call repeatedly; the
   * ref is nulled so a later unmount/clear is a no-op.
   */
  function clearRemotePoll() {
    if (remotePollRef.current !== null) {
      clearInterval(remotePollRef.current);
      remotePollRef.current = null;
    }
  }

  // When the route changes (user clicks a different session), reload history.
  useEffect(() => {
    if (routeId && routeId === selfNavigatedSessionRef.current) {
      // send() created this session and navigated here itself; the live
      // turn (bubbles, HotL state, SSE stream) must survive the URL sync.
      selfNavigatedSessionRef.current = null;
      setSessionId(routeId);
      return;
    }
    selfNavigatedSessionRef.current = null;
    setBubbles([]);
    setHotlPending(null);
    setHotlResolved(null);
    setReconnect(null);
    // Feature ⑥ — a session switch resets the remote-running indicator; the
    // status fetch below re-derives it for the newly-opened session.
    clearRemotePoll();
    setRemoteRunning(false);
    setSessionId(routeId);
    // T5.2 — restore the session's sticky mode (execute for a fresh draft).
    setMode(routeId ? getStoredChatMode(routeId) : 'execute');
    if (!routeId) return;
    void (async () => {
      try {
        const msgs = await client.listMessages(routeId);
        setBubbles(msgs.flatMap(messageToBubbles));
      } catch (err) {
        // A 404 means this session no longer exists (commonly a stale
        // localStorage entry left over after a server/DB reset). Don't show a
        // scary "load failed" — prune the dead entry and drop to a fresh chat.
        if ((err as { status?: number }).status === 404) {
          setBubbles([]);
          onSessionMissing?.(routeId);
          navigate('/', { replace: true });
        } else {
          setStatus(interpolate(t.chat.sse.load_failed, { message: (err as Error).message }));
        }
      }
    })();
    // Feature ⑥ — check whether a turn is still running server-side for this
    // session and, if so, surface a non-blocking indicator + a light poll that
    // auto-clears once it finishes. Best-effort: any failure (endpoint absent,
    // network error) is swallowed so the chat is never disrupted.
    void checkRemoteRunning(routeId);
    return clearRemotePoll;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [routeId]);

  useEffect(() => {
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight });
  }, [bubbles]);

  // Keep the composer textarea sized to its content as the draft changes
  // (including when a suggestion chip or a send() clears it back to one row).
  useEffect(() => {
    autoGrow(textareaRef.current);
  }, [draft]);

  // Populate the model picker from the configured providers (aggregate +
  // de-dupe their model lists). Falls back silently to the server default.
  useEffect(() => {
    void client
      .listProviders()
      .then((ps) => {
        // Which models to offer for one provider:
        //  - probed with >=1 reachable model -> exactly that proven subset (this
        //    is what makes the picker show "only models that connect" after the
        //    operator runs a probe in the admin Providers pane);
        //  - keyed but not yet probed (or a probe that found nothing — usually a
        //    transient failure) -> its advertised models, so the picker never
        //    silently empties out on a bad probe;
        //  - key-less and unverified -> nothing. A key-less provider is either an
        //    unconfigured hosted seed (would 401) or a local server (Ollama) that
        //    may not be running; trust it only once a probe confirms a model.
        const offered = (p: LlmProviderView): string[] => {
          if (p.verified_models && p.verified_models.length > 0) return p.verified_models;
          if (p.has_api_key) return p.models;
          return [];
        };
        const all = [...new Set(ps.flatMap(offered))];
        setModels(all);
        // Default to a provider's declared default model when it's still on
        // offer, else the first offered model. Drop a stale persisted choice.
        const preferred =
          ps.flatMap((p) => p.default_for_models).find((m) => all.includes(m)) ?? all[0] ?? '';
        setModel((cur) => (cur && all.includes(cur) ? cur : preferred));
      })
      .catch(() => {
        /* providers endpoint unavailable — keep the server default. */
      });
  }, []);

  // Persist the operator's model choice across reloads.
  useEffect(() => {
    try {
      if (model) localStorage.setItem('xiaoguai.chat.model', model);
    } catch {
      /* best effort */
    }
  }, [model]);

  // Derive the active assistant (team preferred over persona) for the header
  // display AND the team-run gate (`teamId`). The 助手 tab attaches/detaches
  // server-side; this re-reads the session's attachment on load / session
  // switch. Best-effort: any failure falls back to the 通用 display + no team
  // so the chat never breaks (mirrors AssistantTopicPanel's tolerance).
  useEffect(() => {
    let alive = true;
    if (!sessionId) {
      setTeamId(null);
      setAssistantName(t.ui.assistant.general);
      return;
    }
    void (async () => {
      try {
        const team = await client.getSessionTeam(sessionId);
        if (!alive) return;
        if (team) {
          setTeamId(team.id);
          setAssistantName(team.name);
          return;
        }
        const persona = await client.getSessionPersona(sessionId);
        if (!alive) return;
        setTeamId(null);
        setAssistantName(persona?.name ?? t.ui.assistant.general);
      } catch {
        if (!alive) return;
        // Experts subsystem unavailable / network error — neutral fallback.
        setTeamId(null);
        setAssistantName(t.ui.assistant.general);
      }
    })();
    return () => {
      alive = false;
    };
    // Re-derive whenever the session identity changes. `t` only swaps the
    // fallback language; reloading on locale change is noise.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  // Feature ⑥ — keep the streaming ref in sync for the status poll closure, and
  // drop the remote-running indicator the moment this tab streams a turn (a
  // local turn supersedes the "running elsewhere" cue).
  useEffect(() => {
    streamingRef.current = streaming;
    if (streaming) {
      clearRemotePoll();
      setRemoteRunning(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [streaming]);

  // Phase 4c — Skills "Use in chat" deep-link (`?team=<pack-slug>`). The header
  // ExpertPicker that used to resolve this was removed (the 助手 tab is now the
  // selector), so ChatPage handles it directly: with a session, resolve the
  // slug to the pack's activated team and attach it (then re-derive the header
  // + team-run gate); with no session yet, show a hint to send a message first,
  // then pick the team in the 助手 tab. Consumed once (the `?team=` param is
  // dropped) so a reload / session switch doesn't re-trigger. Best-effort:
  // any failure falls back to the hint and never breaks the chat.
  useEffect(() => {
    const slug = deepLinkTeamSlug?.trim();
    if (!slug) return;
    if (!sessionId) {
      setStatus(interpolate(t.ui.expert.deeplink_need_session, { team: slug }));
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const teams = await client.listTeams();
        if (cancelled) return;
        const team = teamForPackSlug(teams, slug);
        if (!team) return;
        await client.attachSessionTeam(sessionId, team.id);
        if (cancelled) return;
        setTeamId(team.id);
        setAssistantName(team.name);
        setStatus(interpolate(t.ui.expert.deeplink_attached, { team: team.name }));
      } catch {
        // Experts subsystem unavailable / attach failed — point the operator
        // at the 助手 tab instead of failing silently.
        if (!cancelled) {
          setStatus(interpolate(t.ui.expert.deeplink_need_session, { team: slug }));
        }
      } finally {
        if (!cancelled) clearDeepLink();
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [deepLinkTeamSlug, sessionId]);

  /**
   * Feature ⑥ — best-effort: ask the backend whether a turn is still running
   * server-side for `id`. When it is (and this tab isn't locally streaming),
   * raise the `remoteRunning` indicator and start a light ~5s poll that clears
   * it once the turn finishes (its result is already persisted, so a normal
   * history reload shows it). Any failure is swallowed — the chat must never
   * break because the status read failed, and older backends may not expose
   * the endpoint or the client method at all.
   */
  async function checkRemoteRunning(id: string) {
    const getStatus = client.getSessionStatus?.bind(client);
    if (!getStatus) return;
    let inFlight = false;
    try {
      const res = await getStatus(id);
      inFlight = res.in_flight === true;
    } catch {
      // Endpoint missing / network error — skip the indicator entirely.
      return;
    }
    // A local turn may have started while the fetch was in flight; never
    // override the active-streaming UI with the remote indicator.
    if (!inFlight || streamingRef.current) {
      setRemoteRunning(false);
      return;
    }
    setRemoteRunning(true);
    // Light poll: re-read status every ~5s and auto-clear when it finishes or
    // a local turn starts. Only one poll runs at a time.
    clearRemotePoll();
    remotePollRef.current = setInterval(() => {
      if (streamingRef.current) {
        clearRemotePoll();
        setRemoteRunning(false);
        return;
      }
      void getStatus(id)
        .then((r) => {
          if (!r.in_flight) {
            clearRemotePoll();
            setRemoteRunning(false);
          }
        })
        .catch(() => {
          // Transient read failure — keep the indicator; the next tick retries.
        });
    }, 5000);
  }

  /** Phase 4c — drop the consumed `?team=` param (keep other params intact). */
  function clearDeepLink() {
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        next.delete('team');
        return next;
      },
      { replace: true },
    );
  }

  async function send(textOverride?: string) {
    const text = (textOverride ?? draft).trim();
    if (!text || streaming || orchestrating) return;

    // L2b — intercept `/loop …` before it reaches the agent. These commands
    // manage session-scoped recurring loops locally; they are never sent as a
    // chat message. Non-/loop drafts fall through to the normal send path.
    const loopCommand = parseLoopCommand(text);
    if (loopCommand.kind !== 'none') {
      setDraft('');
      await handleLoopCommand(loopCommand, text);
      return;
    }

    let sid = sessionId;
    if (!sid) {
      try {
        const session = await client.createSession({
          user_id: DEV_USER_ID,
          model: model || DEFAULT_MODEL,
          title: text.slice(0, 40),
        });
        sid = session.id;
        setSessionId(sid);
        // T5.2 — carry the pre-session mode choice onto the new session key.
        setStoredChatMode(sid, mode);
        onSessionCreated({ id: sid, title: session.title ?? text.slice(0, 40) });
        // Phase 2 — attach the assistant (persona/team) the operator pre-picked
        // in the panel for this new chat, BEFORE the first turn runs so its
        // system prompt applies from the start. Best-effort (never rejects).
        if (onSessionAttached) await onSessionAttached(sid);
        // Mark this navigation as self-initiated so the route-change effect
        // does not wipe the in-flight turn state (see selfNavigatedSessionRef).
        selfNavigatedSessionRef.current = sid;
        navigate(`/sessions/${sid}`, { replace: true });
      } catch (err) {
        setStatus(`create session failed: ${(err as Error).message}`);
        return;
      }
    }

    setBubbles((bs) => [...bs, { kind: 'user', text }]);
    setBubbles((bs) => [...bs, { kind: 'assistant', text: '', streaming: true }]);
    setDraft('');
    setStreaming(true);
    setStatus(null);
    reconnectingRef.current = false;

    abortRef.current = client.sendMessage(
      sid,
      // Carry the picked model as model_override so the picker applies to EVERY
      // turn — including an existing session whose stored model differs (the
      // picker used to only affect session creation). Only consult goes on the
      // wire; execute is the backend default.
      mode === 'consult'
        ? { content: text, model: model || undefined, mode: 'consult' }
        : { content: text, model: model || undefined },
      (ev) =>
        applyEvent(
          ev,
          setBubbles,
          setStatus,
          setHotlPending,
          setHotlResolved,
          setReconnect,
        ),
      (err) => {
        setStatus(interpolate(t.chat.sse.stream_error, { message: err.message }));
        setStreaming(false);
        setReconnect(null);
      },
      {
        onReconnect: (attempt, delayMs) => {
          reconnectingRef.current = true;
          setReconnect({ attempt, delayMs });
        },
      },
    );

    // Server emits a final `done` event; we flip streaming off there too.
    // Defensive timer keeps the UI unstuck if the connection just drops.
  }

  /** Append a locally-generated system bubble (help / status / error). */
  function pushSystemBubble(textValue: string, extra?: Partial<DisplayBubble>) {
    setBubbles((bs) => [...bs, { kind: 'system', text: textValue, ...extra }]);
  }

  /** Render a loop API failure as the localized, message-bearing error line. */
  function loopErrorText(err: unknown): string {
    return interpolate(t.chat.loop.error, { message: (err as Error).message });
  }

  /**
   * L2b — handle an intercepted `/loop` command. Echoes the operator's input
   * as a user bubble, then drives the matching client call and renders the
   * outcome as a system bubble. Never calls `sendMessage`.
   */
  async function handleLoopCommand(cmd: LoopCommand, raw: string) {
    setStatus(null);
    setBubbles((bs) => [...bs, { kind: 'user', text: raw }]);

    if (cmd.kind === 'help') {
      pushSystemBubble(
        [
          t.chat.loop.help_title,
          t.chat.loop.help_create,
          t.chat.loop.help_status,
          t.chat.loop.help_cancel,
          t.chat.loop.help_help,
        ].join('\n'),
      );
      return;
    }

    const sid = sessionId;
    if (!sid) {
      pushSystemBubble(t.chat.loop.need_session);
      return;
    }

    if (cmd.kind === 'create') {
      pushSystemBubble(
        [
          t.chat.loop.confirm_title,
          interpolate(t.chat.loop.confirm_prompt, { prompt: cmd.prompt }),
          interpolate(t.chat.loop.confirm_pacing, {
            interval: LOOP_DEFAULT_INTERVAL_SECS,
            ticks: LOOP_DEFAULT_MAX_TICKS,
            ttl: LOOP_DEFAULT_TTL_HOURS,
          }),
        ].join('\n'),
        { loopConfirm: { prompt: cmd.prompt } },
      );
      return;
    }

    if (cmd.kind === 'status') {
      try {
        const loops = await client.listLoops();
        const mine = loops.filter((l) => l.session_id === sid);
        if (mine.length === 0) {
          pushSystemBubble(t.chat.loop.status_empty);
        } else {
          pushSystemBubble(formatLoopStatus(mine, t.chat.loop));
        }
      } catch (err) {
        pushSystemBubble(loopErrorText(err));
      }
      return;
    }

    if (cmd.kind !== 'cancel') return; // 'none' is filtered in send(); narrows the union.
    try {
      let id = cmd.id;
      if (!id) {
        const loops = await client.listLoops();
        const live = loops.find((l) => l.session_id === sid && isLoopLive(l));
        if (!live) {
          pushSystemBubble(t.chat.loop.cancel_none);
          return;
        }
        id = live.id;
      }
      const row = await client.cancelLoop(id);
      pushSystemBubble(
        interpolate(t.chat.loop.cancelled, {
          id: shortLoopId(row.id),
          status: row.status,
        }),
      );
    } catch (err) {
      pushSystemBubble(loopErrorText(err));
    }
  }

  /** Arm the loop confirmed at `index`; clears the bubble's Arm/Cancel actions. */
  async function armLoop(index: number, prompt: string) {
    setBubbles((bs) =>
      bs.map((b, j) => (j === index ? { ...b, loopConfirm: undefined } : b)),
    );
    const sid = sessionId;
    if (!sid) {
      pushSystemBubble(t.chat.loop.need_session);
      return;
    }
    try {
      const loop = await client.createLoop({ session_id: sid, prompt });
      pushSystemBubble(
        interpolate(t.chat.loop.armed, {
          id: shortLoopId(loop.id),
          secs: loop.interval_secs,
        }),
      );
    } catch (err) {
      pushSystemBubble(loopErrorText(err));
    }
  }

  /** Dismiss the loop confirmation at `index` without arming. */
  function dismissLoopConfirm(index: number) {
    setBubbles((bs) =>
      bs.map((b, j) => (j === index ? { ...b, loopConfirm: undefined } : b)),
    );
    pushSystemBubble(t.chat.loop.not_armed);
  }

  /** T5.2 — switch consult/execute; persists per session (localStorage). */
  function changeMode(next: TurnMode) {
    setMode(next);
    if (sessionId) setStoredChatMode(sessionId, next);
  }

  /**
   * T5.2 — "团队并行执行": run the current draft as a goal through the
   * attached team via `orchestrateSession`. Member progress renders as a
   * live-updating system bubble; the synthesized text is appended as the
   * assistant bubble on `final{ok:true}`.
   *
   * Double-render avoidance: the backend persists the synthesized reply,
   * so the live assistant bubble carries no `messageId` and the next
   * history reload REPLACES the whole bubble list (route effect) — the
   * exact semantics normal streamed turns already rely on.
   *
   * Always execute mode (plan §2.4) — the entry is disabled in consult.
   */
  async function runTeam() {
    const goal = draft.trim();
    const sid = sessionId;
    const tid = teamId;
    if (!goal || !sid || !tid || streaming || orchestrating || mode === 'consult') {
      return;
    }
    setDraft('');
    setStatus(null);
    setOrchestrating(true);
    // Feature ⑥ — a local team run supersedes the remote-running cue.
    clearRemotePoll();
    setRemoteRunning(false);
    setBubbles((bs) => [...bs, { kind: 'user', text: goal }]);
    // Capture the progress bubble's index so events can update it in place.
    let progressIdx = -1;
    setBubbles((bs) => {
      progressIdx = bs.length;
      return [...bs, { kind: 'system', text: interpolate(t.ui.teamrun.started, { total: '…' }) }];
    });
    const setProgress = (text: string) => {
      setBubbles((bs) => bs.map((b, i) => (i === progressIdx ? { ...b, text } : b)));
    };

    let total = 0;
    let completed = 0;
    try {
      const final = await client.orchestrateSession(sid, { goal, team_id: tid }, (ev: OrchestrateEvent) => {
        switch (ev.type) {
          case 'run_started':
            total = ev.members;
            setProgress(interpolate(t.ui.teamrun.progress, { done: 0, total }));
            break;
          case 'member_started':
            // Counted implicitly — the progress line tracks completions.
            break;
          case 'member_completed':
            completed += 1;
            setProgress(interpolate(t.ui.teamrun.progress, { done: completed, total }));
            break;
          case 'synthesis_started':
            setProgress(interpolate(t.ui.teamrun.synthesizing, { ok: ev.ok_members }));
            break;
          case 'final':
            if (ev.ok) {
              setProgress(t.ui.teamrun.done);
              setBubbles((bs) => [...bs, { kind: 'assistant', text: ev.text }]);
            } else {
              setProgress(
                interpolate(t.ui.teamrun.failed, {
                  failed: ev.failed_members.join(', ') || '—',
                }),
              );
            }
            break;
        }
      });
      if (final === null) {
        // Stream ended without a terminal frame; the detached server task
        // still completes — a history reload will show the persisted reply.
        setProgress(interpolate(t.ui.teamrun.error, { message: 'stream ended early' }));
      }
    } catch (err) {
      // 409 turn-in-flight / 422 / 503 etc. — inline, like other chat errors.
      setProgress(interpolate(t.ui.teamrun.error, { message: (err as Error).message }));
    } finally {
      setOrchestrating(false);
    }
  }

  function applyEvent(
    ev: AgentEvent,
    update: typeof setBubbles,
    statusSetter: typeof setStatus,
    hotlSetter: typeof setHotlPending,
    hotlResolvedSetter: typeof setHotlResolved,
    reconnectSetter: typeof setReconnect,
  ) {
    // Any incoming event = the stream has resumed; tear down the banner.
    reconnectSetter(null);
    // F5 — first event after a reconnect: the backend has no SSE resume, so
    // it re-runs the whole turn from scratch. Roll the in-flight turn back to
    // the user's last message (dropping any partial assistant text and any
    // tool bubbles from the interrupted attempt) and start a fresh streaming
    // bubble, so the re-generated content replaces rather than duplicates.
    // (Amends DEC-LLD-CHAT-UI-003.)
    if (reconnectingRef.current) {
      reconnectingRef.current = false;
      update((bs) => {
        let lastUser = -1;
        for (let i = bs.length - 1; i >= 0; i -= 1) {
          if (bs[i]!.kind === 'user') {
            lastUser = i;
            break;
          }
        }
        const kept = lastUser >= 0 ? bs.slice(0, lastUser + 1) : bs.slice();
        return [...kept, { kind: 'assistant', text: '', streaming: true }];
      });
    }
    switch (ev.type) {
      case 'text_delta':
        update((bs) => {
          const next = [...bs];
          const last = next[next.length - 1];
          if (last && last.streaming && last.kind === 'assistant') {
            next[next.length - 1] = { ...last, text: last.text + ev.delta };
          } else {
            next.push({ kind: 'assistant', text: ev.delta, streaming: true });
          }
          return next;
        });
        break;
      case 'tool_call_started':
        update((bs) => [
          ...bs,
          {
            kind: 'tool',
            text: `→ ${ev.name}(${JSON.stringify(ev.arguments)})`,
            toolName: ev.name,
          },
        ]);
        // Start a fresh assistant bubble after a tool call.
        update((bs) => [...bs, { kind: 'assistant', text: '', streaming: true }]);
        break;
      case 'tool_call_finished':
        update((bs) => [
          ...bs,
          {
            kind: 'tool',
            text: ev.ok
              ? `← ${ev.name}: ${ev.output_text ?? '(no output)'}`
              : `✗ ${ev.name}: ${ev.error ?? 'failed'}`,
            toolName: ev.name,
            toolError: !ev.ok,
          },
        ]);
        break;
      case 'iteration_completed':
        // mark the trailing assistant bubble as no longer streaming
        update((bs) => {
          const next = [...bs];
          const last = next[next.length - 1];
          if (last && last.streaming) {
            next[next.length - 1] = { ...last, streaming: false };
          }
          return next;
        });
        break;
      case 'done':
        update((bs) => {
          const next = [...bs];
          const last = next[next.length - 1];
          if (last && last.streaming) {
            next[next.length - 1] = { ...last, streaming: false };
          }
          return next;
        });
        statusSetter(`done · ${ev.stop_reason}`);
        setStreaming(false);
        break;
      case 'error':
        statusSetter(`agent error: ${ev.message}`);
        setStreaming(false);
        break;
      // sprint-12 S12-8 — HotL suspend/resume events
      case 'hotl_pending':
        hotlSetter({
          escalation_id: ev.escalation_id,
          tool: ev.tool,
          scope: ev.scope,
          args_redacted: ev.args_redacted,
          expires_at: ev.expires_at,
        });
        // Fresh pending — discard any stale resolved event from prior round.
        hotlResolvedSetter(null);
        break;
      case 'hotl_resolved':
        // Primary clear signal — HotlBanner reacts to this via its
        // `resolved` prop and calls `onCleared()`. Per lld-chat-ui §4.3.2.
        hotlResolvedSetter(ev);
        break;
      // v1.3.x — outcome recorded events are informational; no UI change needed here.
      case 'outcome_recorded':
        break;
    }
  }

  function cancel() {
    if (!sessionId) return;
    void client.cancel(sessionId).catch((err) => {
      setStatus(`cancel failed: ${(err as Error).message}`);
    });
    // A user-initiated abort cuts the SSE stream client-side and does NOT fire
    // the stream's `done`/error callbacks — so reset the turn state here.
    // Without this, `streaming` stays true, the Send button never returns, and
    // the composer is stuck (you can't send the next message after Stop).
    abortRef.current?.();
    abortRef.current = null;
    setReconnect(null);
    setStreaming(false);
    // Finalize the trailing assistant bubble so its "thinking" dots stop
    // (mirrors the normal done/error path).
    setBubbles((bs) => {
      const last = bs[bs.length - 1];
      if (last && last.streaming) {
        const next = bs.slice();
        next[next.length - 1] = { ...last, streaming: false };
        return next;
      }
      return bs;
    });
  }

  async function fork(messageId: string) {
    if (!sessionId) return;
    try {
      const child = await client.forkSession(sessionId, { from_message_id: messageId });
      // Open in a new tab — preserves the user's current spot in the
      // original session, which is the whole point of branching.
      window.open(`/sessions/${child.id}`, '_blank', 'noopener');
    } catch (err) {
      setStatus(`fork failed: ${(err as Error).message}`);
    }
  }

  /**
   * Phase 4b — copy a bubble's text to the clipboard. Best-effort: a clipboard
   * write can reject (insecure context / denied permission), so surface the
   * failure in the status line rather than swallowing it.
   */
  async function copyBubble(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      setStatus(t.ui.message_actions.copied);
    } catch (err) {
      setStatus(interpolate(t.ui.message_actions.copy_failed, { message: (err as Error).message }));
    }
  }

  /**
   * Phase 4b — delete one persisted message (confirmed) and drop its bubble.
   * Only callable when the bubble carries a `messageId` and a session exists.
   * Best-effort: on failure the bubble stays and the error is surfaced.
   */
  async function deleteBubble(messageId: string) {
    const sid = sessionId;
    if (!sid || streaming || orchestrating) return;
    if (!window.confirm(t.ui.message_actions.delete_confirm)) return;
    try {
      await client.deleteMessage(sid, messageId);
      setStatus(null);
      // Immutable removal: rebuild the list without any bubble carrying this id.
      setBubbles((bs) => bs.filter((b) => b.messageId !== messageId));
    } catch (err) {
      setStatus(
        interpolate(t.ui.message_actions.delete_failed, { message: (err as Error).message }),
      );
    }
  }

  /**
   * Phase 4b — regenerate the latest response. Deletes the last assistant
   * message AND its preceding user message, then re-sends the user text so a
   * fresh response replaces the last exchange. If either delete fails we abort
   * WITHOUT resending (surfacing the error), so we never duplicate the turn.
   * This bounds the deletes to the last exchange only.
   *
   * `assistantId` / `userId` are the persisted ids captured at render time on
   * the last assistant bubble + the user bubble right before it.
   */
  async function regenerateLast(assistantId: string, userId: string, userText: string) {
    const sid = sessionId;
    if (!sid || streaming || orchestrating) return;
    try {
      // Delete the assistant reply first, then the user turn that prompted it.
      await client.deleteMessage(sid, assistantId);
      await client.deleteMessage(sid, userId);
    } catch (err) {
      setStatus(
        interpolate(t.ui.message_actions.regenerate_failed, { message: (err as Error).message }),
      );
      return;
    }
    // Drop the deleted exchange locally before the resend appends fresh bubbles.
    setBubbles((bs) => bs.filter((b) => b.messageId !== assistantId && b.messageId !== userId));
    await send(userText);
  }

  /**
   * Phase 4b — edit the last user message. Prompts for new text (seeded with
   * the current text — the chat-ui has no modal layer), deletes that user
   * message (and the trailing assistant reply if present), then re-sends the
   * edited text as a fresh turn. Cancel / empty input is a no-op. Aborts on a
   * delete failure WITHOUT resending, like regenerate.
   *
   * `assistantId` is the id of the assistant reply that followed this user
   * message (when present), so it can be cleared alongside.
   */
  async function editLastUser(userId: string, currentText: string, assistantId?: string) {
    const sid = sessionId;
    if (!sid || streaming || orchestrating) return;
    const edited = window.prompt(t.ui.message_actions.edit_prompt, currentText);
    if (edited === null) return; // cancelled
    const trimmed = edited.trim();
    if (!trimmed) return; // empty → no-op
    try {
      await client.deleteMessage(sid, userId);
      if (assistantId) await client.deleteMessage(sid, assistantId);
    } catch (err) {
      setStatus(
        interpolate(t.ui.message_actions.edit_failed, { message: (err as Error).message }),
      );
      return;
    }
    setBubbles((bs) =>
      bs.filter((b) => b.messageId !== userId && (!assistantId || b.messageId !== assistantId)),
    );
    await send(trimmed);
  }

  // Phase 4b — derive which bubble (if any) gets the "regenerate" action and
  // which gets "edit". Both are bounded to the LAST exchange so we never offer
  // an unbounded "delete everything after". Computed each render from the
  // current bubble list (cheap; no extra state).
  //
  // regenerate: the last assistant bubble that has a persisted id and isn't
  // mid-stream, but only when the user message right before it also has an id —
  // both are needed to delete the exchange and replay the user text.
  const regenInfo = (() => {
    if (streaming || orchestrating || !sessionId) return undefined;
    let aIdx = -1;
    for (let i = bubbles.length - 1; i >= 0; i -= 1) {
      const b = bubbles[i]!;
      if (b.kind === 'assistant' && b.messageId && !b.streaming) {
        aIdx = i;
        break;
      }
    }
    if (aIdx < 0) return undefined;
    // Walk back to the nearest preceding user bubble carrying an id.
    for (let j = aIdx - 1; j >= 0; j -= 1) {
      const u = bubbles[j]!;
      if (u.kind === 'user' && u.messageId) {
        return {
          assistantIdx: aIdx,
          assistantId: bubbles[aIdx]!.messageId!,
          userId: u.messageId,
          userText: u.text,
        };
      }
    }
    return undefined;
  })();

  // edit: the last user bubble that has a persisted id, plus the trailing
  // assistant reply's id (if any) so the whole exchange clears on resend.
  const editInfo = (() => {
    if (streaming || orchestrating || !sessionId) return undefined;
    let uIdx = -1;
    for (let i = bubbles.length - 1; i >= 0; i -= 1) {
      const b = bubbles[i]!;
      if (b.kind === 'user' && b.messageId) {
        uIdx = i;
        break;
      }
    }
    if (uIdx < 0) return undefined;
    // The reply (if any) is the first assistant bubble after the user message.
    let replyId: string | undefined;
    for (let j = uIdx + 1; j < bubbles.length; j += 1) {
      const r = bubbles[j]!;
      if (r.kind === 'assistant' && r.messageId) {
        replyId = r.messageId;
        break;
      }
    }
    return {
      userIdx: uIdx,
      userId: bubbles[uIdx]!.messageId!,
      userText: bubbles[uIdx]!.text,
      assistantId: replyId,
    };
  })();

  return (
    <>
      {/* sprint-12 S12-8 — HotL suspend/resume banner: non-dismissible, shown
          above messages. The PRIMARY clear signal is the matching
          `hotl_resolved` SSE event reduced into `hotlResolved`; HotlBanner
          subscribes to it via the `resolved` prop and calls `onCleared` to
          unmount. A defensive 30 s fallback inside the banner covers the
          SSE-interrupted case. See lld-chat-ui.md §4.3.2. */}
      {hotlPending && (
        <HotlBanner
          pending={hotlPending}
          resolved={hotlResolved}
          decidedBy="chat-ui"
          onCleared={() => {
            setHotlPending(null);
            setHotlResolved(null);
          }}
          onDecision={async (verdict, raisePolicy) => {
            const res = await client.submitHotlDecision({
              escalation_id: hotlPending.escalation_id,
              verdict,
              decided_by: 'chat-ui',
              raise_policy: raisePolicy,
            });
            // HotlBanner uses `resumed` to decide its clear path: when
            // `resumed:false` no `hotl_resolved` SSE will arrive (no loop
            // was suspended — the v1.8.x norm), so it clears optimistically
            // rather than waiting the 30 s fallback. `resumed:true` keeps
            // the SSE-primary + fallback path.
            return { resumed: res.resumed };
          }}
        />
      )}
      {/* sprint-11 S11-2b — SSE reconnect banner (LLD-CHAT-UI-001 §4.7.1). */}
      {reconnect && (
        <SseReconnectBanner
          attempt={reconnect.attempt}
          nextDelayMs={reconnect.delayMs}
          onCancel={cancel}
        />
      )}
      {/* Phase 3 (Cherry-Studio IA) — chat-area top bar: read-only
          active-assistant display + watch / remote-running cues. The assistant
          is now SELECTED in the 助手 tab of the list panel; the header only
          reflects what's attached (the old ExpertPicker popover rendered hidden
          here and was redundant with that tab). The model selector lives in the
          composer meta row (after the mode toggle). */}
      <ChatHeaderBar
        assistantName={assistantName}
        remoteRunning={
          /* Feature ⑥ — non-blocking cue: a turn is still running server-side
             for this session, but this tab isn't streaming it. Its result is
             persisted on completion; a history reload then shows it. */
          remoteRunning && !streaming ? (
            <span
              className="remote-running"
              role="status"
              title={t.chat.remote_running_title}
              data-testid="remote-running"
            >
              <span className="remote-running__dot" aria-hidden="true" />
              {t.chat.remote_running}
            </span>
          ) : null
        }
        watch={<WatchIndicator sessionId={sessionId} />}
      />
      <div className={`messages${bubbles.length === 0 ? ' messages-empty' : ''}`} ref={scrollRef}>
        {bubbles.length === 0 ? (
          <div className="welcome">
            <h1 className="welcome-title">
              {interpolate(t.ui.welcome_title, { name: brandName })}
            </h1>
            <p className="welcome-subtitle">{t.ui.welcome_subtitle}</p>
            <div className="welcome-chips">
              {[
                t.ui.suggestions.summarize_doc,
                t.ui.suggestions.write_shell,
                t.ui.suggestions.analyze_cve,
                t.ui.suggestions.explain_codebase,
              ].map((prompt) => (
                <button
                  key={prompt}
                  type="button"
                  className="suggestion-chip"
                  onClick={() => void send(prompt)}
                >
                  {prompt}
                </button>
              ))}
            </div>
          </div>
        ) : (
          bubbles.map((b, i) => (
            <Bubble
              key={i}
              index={i}
              bubble={b}
              regenerate={i === regenInfo?.assistantIdx ? regenInfo : undefined}
              edit={i === editInfo?.userIdx ? editInfo : undefined}
              canDelete={!streaming && !orchestrating && !!sessionId}
              onFork={fork}
              onCopy={copyBubble}
              onDelete={deleteBubble}
              onRegenerate={regenerateLast}
              onEdit={editLastUser}
              onLoopArm={armLoop}
              onLoopCancel={dismissLoopConfirm}
            />
          ))
        )}
      </div>
      {status && <div className="status">{status}</div>}
      {/* Phase 4c — onboarding cue when a team is attached but the composer is
          empty: a team is ready, hand it a complex goal. The seeded example
          fills the draft on click so the operator can run it (or edit first). */}
      {teamId && !draft.trim() && !streaming && !orchestrating && mode !== 'consult' && (
        <div className="teamrun-hint" data-testid="teamrun-hint">
          <span className="teamrun-hint__lead">{t.ui.teamrun.button_title}</span>
          <button
            type="button"
            className="teamrun-hint__example"
            onClick={() => {
              setDraft(t.ui.teamrun.example_goal);
              textareaRef.current?.focus();
            }}
            title={t.ui.teamrun.example_goal}
          >
            {t.ui.teamrun.example_goal}
          </button>
        </div>
      )}
      <div className="composer">
        <div className={`composer-box${mode === 'consult' ? ' composer-box--consult' : ''}`}>
          <textarea
            ref={textareaRef}
            className="composer-input"
            value={draft}
            rows={1}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault();
                void send();
              }
            }}
            placeholder={interpolate(t.ui.composer_placeholder, { name: brandName })}
          />
          {/* T5.2 — team parallel run: only when a team is attached. Always
              execute mode, so consult disables it (tooltip explains). */}
          {teamId && !streaming && (
            <button
              type="button"
              className="teamrun-btn"
              onClick={() => void runTeam()}
              disabled={mode === 'consult' || orchestrating || !draft.trim()}
              title={
                mode === 'consult'
                  ? t.ui.teamrun.disabled_consult
                  : t.ui.teamrun.button_title
              }
              data-testid="teamrun-btn"
            >
              {t.ui.teamrun.button}
            </button>
          )}
          {streaming ? (
            <button
              type="button"
              className="composer-btn stop"
              onClick={cancel}
              aria-label={t.ui.stop_generating}
              title={t.ui.stop}
            >
              <StopIcon />
            </button>
          ) : (
            <button
              type="button"
              className="composer-btn send"
              onClick={() => void send()}
              disabled={!draft.trim() || orchestrating}
              aria-label={t.ui.send_message}
              title={t.ui.send}
            >
              <SendIcon />
            </button>
          )}
        </div>
        <div className="composer-meta">
          {/* T5.2 — execute / read-only toggle (read-only cue is a tooltip). */}
          <ModeToggle mode={mode} onChange={changeMode} />
          {/* Model selector — moved here from the chat top bar so it follows the
              mode toggle. State / send-path (`model_override`) are unchanged;
              only the render location moved. Hidden when no models are offered
              (same rule as before); keeps `aria-label="model"` for the test. */}
          {models.length > 0 && (
            <label className="chat-model-select">
              <span className="chat-model-select__label">{t.ui.header.model_label}</span>
              <select
                className="chat-model-select__control"
                value={model}
                onChange={(e) => setModel(e.target.value)}
                aria-label="model"
                title="model"
              >
                {models.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </select>
            </label>
          )}
          <div className="composer-hint">{t.ui.composer_hint}</div>
        </div>
      </div>
    </>
  );
}

/** Up-arrow send glyph (inline SVG — no icon dependency). */
function SendIcon() {
  return (
    <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true" focusable="false">
      <path
        d="M12 19V5M12 5l-6 6M12 5l6 6"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/** Filled square stop glyph. */
function StopIcon() {
  return (
    <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true" focusable="false">
      <rect x="6" y="6" width="12" height="12" rx="2" fill="currentColor" />
    </svg>
  );
}

/** Phase 4b — the last-exchange "regenerate" descriptor passed to the matching bubble. */
interface RegenInfo {
  assistantIdx: number;
  assistantId: string;
  userId: string;
  userText: string;
}

/** Phase 4b — the last-user "edit" descriptor passed to the matching bubble. */
interface EditInfo {
  userIdx: number;
  userId: string;
  userText: string;
  assistantId?: string;
}

function Bubble({
  bubble,
  index,
  regenerate,
  edit,
  canDelete,
  onFork,
  onCopy,
  onDelete,
  onRegenerate,
  onEdit,
  onLoopArm,
  onLoopCancel,
}: {
  bubble: DisplayBubble;
  index: number;
  /** Present only on the bubble that owns the last-exchange regenerate action. */
  regenerate?: RegenInfo;
  /** Present only on the bubble that owns the last-user edit action. */
  edit?: EditInfo;
  /** Whether delete is currently allowed (no in-flight turn + a session exists). */
  canDelete: boolean;
  onFork: (messageId: string) => void;
  onCopy: (text: string) => void;
  onDelete: (messageId: string) => void;
  onRegenerate: (assistantId: string, userId: string, userText: string) => void;
  onEdit: (userId: string, currentText: string, assistantId?: string) => void;
  onLoopArm: (index: number, prompt: string) => void;
  onLoopCancel: (index: number) => void;
}) {
  const { t } = useI18n();
  const className =
    bubble.kind === 'tool'
      ? `bubble tool copy-host${bubble.toolError ? ' error' : ''}`
      : `bubble ${bubble.kind} copy-host`;
  const isEmptyStreaming = !bubble.text && bubble.streaming;
  // v0.8.2: assistant turns render through react-markdown so headings,
  // tables, fenced code blocks, etc. come out formatted. User + tool
  // turns stay as raw text — they're not authored as markdown.
  const renderMarkdown = bubble.kind === 'assistant' && !isEmptyStreaming && bubble.text;
  // v0.8.3: tool bubbles are machine output and frequently copied verbatim
  // into shells / issue trackers, so they get the same hover-to-copy
  // affordance code blocks do. Empty / still-streaming bubbles skip it.
  const showCopy = bubble.kind === 'tool' && bubble.text.length > 0;
  // Phase 4b: the per-message hover toolbar (copy / regenerate / edit / branch
  // / delete) is shown on chat turns only — never on tool output or locally-
  // generated system bubbles, and not while this bubble is still streaming
  // (its id isn't persisted yet). messageId-dependent actions stay gated on a
  // persisted id, mirroring the legacy "Branch from here" affordance.
  const isChatTurn = bubble.kind === 'user' || bubble.kind === 'assistant';
  const showToolbar = isChatTurn && bubble.text.length > 0 && !bubble.streaming;
  const hasId = !!bubble.messageId;
  return (
    <div className={className}>
      {showCopy && <CopyButton text={bubble.text} />}
      {showToolbar && (
        <MessageToolbar
          onCopy={() => onCopy(bubble.text)}
          onBranch={
            // v1.1.2 → Phase 4b: branch keeps its original gate — a persisted
            // id is required, and forking from a user prompt is just "create a
            // new session", so it stays assistant-only.
            bubble.kind === 'assistant' && hasId
              ? () => onFork(bubble.messageId!)
              : undefined
          }
          onDelete={hasId && canDelete ? () => onDelete(bubble.messageId!) : undefined}
          onRegenerate={
            regenerate
              ? () => onRegenerate(regenerate.assistantId, regenerate.userId, regenerate.userText)
              : undefined
          }
          onEdit={
            edit ? () => onEdit(edit.userId, edit.userText, edit.assistantId) : undefined
          }
        />
      )}
      {renderMarkdown ? <MarkdownBody text={bubble.text} /> : bubble.text}
      {isEmptyStreaming && (
        <span className="streaming-dots" aria-label={t.ui.thinking}>
          <span />
          <span />
          <span />
        </span>
      )}
      {/* L2b — Arm / Cancel actions on an un-armed /loop confirmation. */}
      {bubble.loopConfirm && (
        <div className="loop-confirm-actions">
          <button
            type="button"
            className="loop-confirm-btn arm"
            onClick={() => onLoopArm(index, bubble.loopConfirm!.prompt)}
          >
            {t.chat.loop.btn_arm}
          </button>
          <button
            type="button"
            className="loop-confirm-btn cancel"
            onClick={() => onLoopCancel(index)}
          >
            {t.chat.loop.btn_cancel}
          </button>
        </div>
      )}
      {bubble.citations && bubble.citations.length > 0 && (
        <CitationStrip citations={bubble.citations} />
      )}
    </div>
  );
}

/**
 * L2b — render a session's loops as a multi-line status block. Pure so it can
 * be unit-tested; takes just the translation slice it needs.
 */
function formatLoopStatus(
  loops: LoopResponse[],
  loopT: { status_header: string; status_line: string },
): string {
  const lines = loops.map((l) =>
    interpolate(loopT.status_line, {
      id: shortLoopId(l.id),
      status: l.status,
      ran: l.ticks_run,
      max: l.max_ticks,
      next: new Date(l.next_tick_at).toLocaleTimeString(),
    }),
  );
  return [loopT.status_header, ...lines].join('\n');
}

function messageToBubbles(m: Message): DisplayBubble[] {
  const out: DisplayBubble[] = [];
  // v0.9.3: collect every citation in the message and attach the strip
  // to the *last* assistant bubble produced — that's the turn whose
  // text the citations annotate. Pre-extract so the loop body can
  // ignore them.
  const citations = m.content.filter(
    (b): b is CitationBlock => b.type === 'citation',
  );
  let lastAssistantIdx: number | null = null;

  for (const block of m.content) {
    if (block.type === 'text') {
      const idx = out.length;
      out.push({
        kind: m.role === 'user' ? 'user' : 'assistant',
        text: block.text,
        // v1.1.2: tag assistant bubbles with their persisted message id
        // so the "Branch from here" button knows the cutoff. We attach
        // it to *every* text-block we produce from this message — if a
        // multi-block assistant turn renders as N bubbles, branching
        // from any of them cuts at the same boundary, which is the
        // expected behaviour at the schema level.
        //
        // Phase 4b: USER text blocks now also carry their persisted id —
        // the message toolbar's delete / edit / regenerate actions key off
        // it. Tool / system bubbles stay id-less (no toolbar id-actions).
        messageId: m.role === 'user' || m.role === 'assistant' ? m.id : undefined,
      });
      if (m.role !== 'user') lastAssistantIdx = idx;
    } else if (block.type === 'tool_call') {
      out.push({
        kind: 'tool',
        text: `→ ${block.name}(${JSON.stringify(block.arguments)})`,
        toolName: block.name,
      });
    } else if (block.type === 'tool_result') {
      out.push({
        kind: 'tool',
        text: `← ${JSON.stringify(block.output)}`,
        toolError: block.is_error,
      });
    }
    // citation: harvested above; nothing to emit per-block here
  }
  if (citations.length > 0 && lastAssistantIdx !== null) {
    const target = out[lastAssistantIdx];
    if (target) target.citations = citations;
  }
  return out;
}
