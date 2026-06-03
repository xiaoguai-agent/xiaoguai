import { useEffect, useRef, useState } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import type {
  AgentEvent,
  ContentBlock,
  HotlResolvedEvent,
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

type CitationBlock = Extract<ContentBlock, { type: 'citation' }>;

interface Props {
  onSessionCreated: (s: { id: string; title: string }) => void;
}

interface DisplayBubble {
  kind: 'user' | 'assistant' | 'tool';
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
}

const DEV_USER_ID = 'usr_dev';
const DEV_TENANT_ID = 'ten_dev';
const DEFAULT_MODEL = 'qwen2.5-coder';

/** Opening prompts shown on the empty/welcome screen (Gemini-style chips). */
const SUGGESTIONS = [
  'Summarize a document for me',
  'Write a shell script',
  'Analyze the latest CVE feed',
  'Explain how a codebase works',
];

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
        onReconnect: (attempt, delayMs) => setReconnect({ attempt, delayMs }),
      },
    );

    // Server emits a final `done` event; we flip streaming off there too.
    // Defensive timer keeps the UI unstuck if the connection just drops.
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
    // Per DEC-LLD-CHAT-UI-003 the partial bubble itself is never touched.
    reconnectSetter(null);
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
            await client.submitHotlDecision({
              escalation_id: hotlPending.escalation_id,
              verdict,
              decided_by: 'chat-ui',
              raise_policy: raisePolicy,
            });
            // Sprint-12: do NOT unmount the banner here. HotlBanner waits
            // for the matching `hotl_resolved` SSE event (primary signal)
            // or its internal 30 s defensive fallback (SSE-interrupted
            // case) and then invokes `onCleared` above.
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
              {SUGGESTIONS.map((prompt) => (
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
          bubbles.map((b, i) => <Bubble key={i} bubble={b} onFork={fork} />)
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
  onFork,
}: {
  bubble: DisplayBubble;
  onFork: (messageId: string) => void;
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
      {bubble.citations && bubble.citations.length > 0 && (
        <CitationStrip citations={bubble.citations} />
      )}
    </div>
  );
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
