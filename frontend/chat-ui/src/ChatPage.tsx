import { useEffect, useRef, useState } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import type {
  AgentEvent,
  ContentBlock,
  HotlResolvedEvent,
  LoopResponse,
  Message,
} from '@xiaoguai/shared';
import { client } from './client';
import { CitationStrip } from './citations';
import { CopyButton } from './codeblock';
import { MarkdownBody } from './markdown';
import { HotlBanner } from './HotlBanner';
import type { HotlPendingState } from './HotlBanner';
import { AiDisclosureBanner } from './AiDisclosureBanner';
import { SseReconnectBanner } from './SseReconnectBanner';
import { WatchIndicator } from './WatchIndicator';
import { useI18n } from './i18n/I18nProvider';
import { interpolate } from './i18n';
import { isLoopLive, parseLoopCommand, shortLoopId } from './loopCommands';
import type { LoopCommand } from './loopCommands';

type CitationBlock = Extract<ContentBlock, { type: 'citation' }>;

interface Props {
  onSessionCreated: (s: { id: string; title: string }) => void;
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
const DEV_TENANT_ID = 'ten_dev';
const DEFAULT_MODEL = 'qwen2.5-coder';

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

export function ChatPage({ onSessionCreated }: Props) {
  const { t } = useI18n();
  const { id: routeId } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const [sessionId, setSessionId] = useState<string | undefined>(routeId);
  const [bubbles, setBubbles] = useState<DisplayBubble[]>([]);
  const [draft, setDraft] = useState('');
  const [streaming, setStreaming] = useState(false);
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

  // When the route changes (user clicks a different session), reload history.
  useEffect(() => {
    setBubbles([]);
    setHotlPending(null);
    setHotlResolved(null);
    setReconnect(null);
    setSessionId(routeId);
    if (!routeId) return;
    void (async () => {
      try {
        const msgs = await client.listMessages(routeId);
        setBubbles(msgs.flatMap(messageToBubbles));
      } catch (err) {
        setStatus(`load failed: ${(err as Error).message}`);
      }
    })();
  }, [routeId]);

  useEffect(() => {
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight });
  }, [bubbles]);

  // Keep the composer textarea sized to its content as the draft changes
  // (including when a suggestion chip or a send() clears it back to one row).
  useEffect(() => {
    autoGrow(textareaRef.current);
  }, [draft]);

  async function send(textOverride?: string) {
    const text = (textOverride ?? draft).trim();
    if (!text || streaming) return;

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
          model: DEFAULT_MODEL,
          title: text.slice(0, 40),
        });
        sid = session.id;
        setSessionId(sid);
        onSessionCreated({ id: sid, title: session.title ?? text.slice(0, 40) });
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
      { content: text },
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
        setStatus(`stream error: ${err.message}`);
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
    abortRef.current?.();
    setReconnect(null);
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
      {/* AI disclosure banner (EU AI Act Art. 50(1)) — always above HotlBanner */}
      <AiDisclosureBanner tenantId={DEV_TENANT_ID} />
      {/* Chat header — order: AiDisclosureBanner > HotlBanner > WatchIndicator */}
      <div className="chat-header">
        {/* AiDisclosureBanner placeholder — wired by feat/chat-ui-ai-disclosure branch */}
        {/* HotlBanner placeholder — wired by feat/chat-ui-hotl-banner branch */}
        <WatchIndicator sessionId={sessionId} />
      </div>
      <div className={`messages${bubbles.length === 0 ? ' messages-empty' : ''}`} ref={scrollRef}>
        {bubbles.length === 0 ? (
          <div className="welcome">
            <h1 className="welcome-title">{t.ui.welcome_title}</h1>
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
              onFork={fork}
              onLoopArm={armLoop}
              onLoopCancel={dismissLoopConfirm}
            />
          ))
        )}
      </div>
      {status && <div className="status">{status}</div>}
      <div className="composer">
        <div className="composer-box">
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
            placeholder={t.ui.composer_placeholder}
          />
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
              disabled={!draft.trim()}
              aria-label={t.ui.send_message}
              title={t.ui.send}
            >
              <SendIcon />
            </button>
          )}
        </div>
        <div className="composer-hint">{t.ui.composer_hint}</div>
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

function Bubble({
  bubble,
  index,
  onFork,
  onLoopArm,
  onLoopCancel,
}: {
  bubble: DisplayBubble;
  index: number;
  onFork: (messageId: string) => void;
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
  // v1.1.2: branch from here. Only on persisted assistant bubbles
  // (live-streaming ones have no message id yet, and forking from a
  // user prompt is just "create a new session").
  const showFork = bubble.kind === 'assistant' && !!bubble.messageId && !bubble.streaming;
  return (
    <div className={className}>
      {showCopy && <CopyButton text={bubble.text} />}
      {showFork && bubble.messageId && (
        <button
          type="button"
          className="bubble-action bubble-fork"
          title={t.ui.branch_title}
          aria-label={t.ui.branch_label}
          onClick={() => onFork(bubble.messageId!)}
        >
          {t.ui.branch}
        </button>
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
        messageId: m.role === 'assistant' ? m.id : undefined,
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
