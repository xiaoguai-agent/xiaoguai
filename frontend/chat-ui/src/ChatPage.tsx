import { useEffect, useRef, useState } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import type { AgentEvent, ContentBlock, Message } from '@xiaoguai/shared';
import { client } from './client';
import { CitationStrip } from './citations';
import { CopyButton } from './codeblock';
import { MarkdownBody } from './markdown';
import { HotlBanner } from './HotlBanner';
import type { HotlPendingState } from './HotlBanner';
import { AiDisclosureBanner } from './AiDisclosureBanner';

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

export function ChatPage({ onSessionCreated }: Props) {
  const { id: routeId } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const [sessionId, setSessionId] = useState<string | undefined>(routeId);
  const [bubbles, setBubbles] = useState<DisplayBubble[]>([]);
  const [draft, setDraft] = useState('');
  const [streaming, setStreaming] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  /** v1.3.x — non-null while an HotL escalation is pending for this session. */
  const [hotlPending, setHotlPending] = useState<HotlPendingState | null>(null);
  const abortRef = useRef<(() => void) | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);

  // When the route changes (user clicks a different session), reload history.
  useEffect(() => {
    setBubbles([]);
    setHotlPending(null);
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

  async function send() {
    const text = draft.trim();
    if (!text || streaming) return;

    let sid = sessionId;
    if (!sid) {
      try {
        const session = await client.createSession({
          user_id: DEV_USER_ID,
          tenant_id: DEV_TENANT_ID,
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
      (ev) => applyEvent(ev, setBubbles, setStatus, setHotlPending),
      (err) => {
        setStatus(`stream error: ${err.message}`);
        setStreaming(false);
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
  ) {
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
      // v1.3.x — HotL escalation events
      case 'hotl_pending':
        hotlSetter({
          escalation_id: ev.escalation_id,
          scope: ev.scope,
          reason: ev.reason,
        });
        break;
      case 'hotl_resolved':
        // Clear the banner once the operator resolves the escalation.
        hotlSetter(null);
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
      {/* v1.3.x — HotL escalation banner: non-dismissible, shown above messages. */}
      {hotlPending && <HotlBanner pending={hotlPending} />}
      {/* AI disclosure banner (EU AI Act Art. 50(1)) — always above HotlBanner */}
      <AiDisclosureBanner tenantId={DEV_TENANT_ID} />
      <div className="messages" ref={scrollRef}>
        {bubbles.map((b, i) => (
          <Bubble key={i} bubble={b} onFork={fork} />
        ))}
      </div>
      {status && <div className="status">{status}</div>}
      <div className="composer">
        <input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              void send();
            }
          }}
          placeholder="Message Xiaoguai..."
          disabled={streaming}
        />
        {streaming ? (
          <button onClick={cancel}>Cancel</button>
        ) : (
          <button onClick={() => void send()} disabled={!draft.trim()}>
            Send
          </button>
        )}
      </div>
    </>
  );
}

function Bubble({
  bubble,
  onFork,
}: {
  bubble: DisplayBubble;
  onFork: (messageId: string) => void;
}) {
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
          title="Branch a new conversation from this point"
          aria-label="Branch from here"
          onClick={() => onFork(bubble.messageId!)}
        >
          Branch
        </button>
      )}
      {renderMarkdown ? <MarkdownBody text={bubble.text} /> : bubble.text}
      {isEmptyStreaming && (
        <span className="streaming-dots" aria-label="thinking">
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
