/**
 * v0.9.3 — RAG citation chips.
 *
 * Renders one `[n]` chip per citation block attached to an assistant
 * turn. Hovering shows the chunk preview, score, and source URI;
 * clicking opens the source in a new tab.
 *
 * The contract this UI assumes (from `xiaoguai-rag`'s citation lock):
 *   source_uri  — file:// | http(s):// | obsidian:// | r2r://...
 *   span        — [start, end] 1-indexed lines, or [0, 0] when unknown
 *   score       — [0, 1]
 *   preview     — ~200-400 char chunk text
 *   collection_id — for "find more from this collection"
 *
 * Whole-document fallback: `span === [0, 0]` means the backend couldn't
 * anchor to lines (R2R without line metadata, for example). We still
 * render the chip; the link just opens the URI without a `#L<n>`.
 */

import { useState } from 'react';
import { safeHref } from '@xiaoguai/shared';
import type { ContentBlock } from '@xiaoguai/shared';

type CitationBlock = Extract<ContentBlock, { type: 'citation' }>;

interface Props {
  citations: CitationBlock[];
}

export function CitationStrip({ citations }: Props) {
  if (citations.length === 0) return null;
  // Sort by score desc so the strongest source shows up first.
  const sorted = [...citations].sort((a, b) => b.score - a.score);
  return (
    <div className="citation-strip" role="list" aria-label="Sources">
      {sorted.map((c, i) => (
        <CitationChip key={`${c.source_uri}-${c.span[0]}-${i}`} citation={c} index={i + 1} />
      ))}
    </div>
  );
}

function CitationChip({ citation, index }: { citation: CitationBlock; index: number }) {
  const [open, setOpen] = useState(false);
  const href = anchoredHref(citation);
  // Chip opacity tracks score so weaker hits visually recede.
  const opacity = Math.max(0.55, Math.min(1, 0.55 + citation.score * 0.45));

  return (
    <span
      className="citation-chip"
      role="listitem"
      style={{ opacity }}
      onMouseEnter={() => setOpen(true)}
      onMouseLeave={() => setOpen(false)}
      onFocus={() => setOpen(true)}
      onBlur={() => setOpen(false)}
    >
      {href !== undefined ? (
        <a
          href={href}
          target="_blank"
          rel="noreferrer noopener"
          className="citation-chip__link"
          title={`Open ${shortLabel(citation)}`}
        >
          [{index}]
        </a>
      ) : (
        // SEC-24: source_uri failed the protocol whitelist (javascript:/
        // data:/unknown scheme) — keep the chip + tooltip, drop the link.
        <span className="citation-chip__link" title={shortLabel(citation)}>
          [{index}]
        </span>
      )}
      {open && (
        <span className="citation-card" role="tooltip">
          <span className="citation-card__source">{shortLabel(citation)}</span>
          <span className="citation-card__score">score {citation.score.toFixed(2)}</span>
          <span className="citation-card__preview">{citation.preview}</span>
        </span>
      )}
    </span>
  );
}

function anchoredHref(c: CitationBlock): string | undefined {
  // SEC-24: source_uri is backend/LLM-influenced. Only whitelist-approved
  // schemes (http/https/file/mailto + the citation-lock obsidian/r2r) may
  // become a link; anything else renders as plain text.
  const base = safeHref(c.source_uri);
  if (base === undefined) return undefined;
  // `(0, 0)` = no anchor; link to whole document.
  if (c.span[0] === 0 && c.span[1] === 0) return base;
  // GitHub and most viewers support `#L<n>-L<m>`; for `file://` it's
  // ignored but harmless. We deliberately don't browser-special-case
  // each scheme — one consistent shape, predictable for tests.
  return `${base}#L${c.span[0]}-L${c.span[1]}`;
}

function shortLabel(c: CitationBlock): string {
  // file:///long/path/notes.md → notes.md
  // https://github.com/foo/bar/blob/main/src/lib.rs → bar/src/lib.rs
  try {
    const u = new URL(c.source_uri);
    if (u.protocol === 'file:') {
      const parts = u.pathname.split('/').filter(Boolean);
      return parts[parts.length - 1] ?? c.source_uri;
    }
    return `${u.host}${u.pathname}`;
  } catch {
    return c.source_uri;
  }
}
