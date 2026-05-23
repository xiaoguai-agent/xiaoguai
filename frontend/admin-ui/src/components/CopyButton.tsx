/**
 * Small clipboard-copy button. Mirrors the v0.8.3 chat-ui `CopyButton`
 * (frontend/chat-ui/src/codeblock.tsx) — duplicated here rather than
 * imported because admin-ui doesn't depend on chat-ui and lifting the
 * component into `@xiaoguai/shared` would drag React into a types-only
 * package. Keep the two in sync if either grows behaviour.
 */

import { useState } from 'react';
import type { ReactNode } from 'react';

interface CopyButtonProps {
  text: string;
  label?: ReactNode;
}

export function CopyButton({ text, label = 'Copy' }: CopyButtonProps) {
  const [copied, setCopied] = useState(false);

  async function onClick() {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1200);
    } catch {
      // Clipboard API rejects in insecure contexts / when permission is
      // denied. Surface a brief failure label instead of silently no-op.
      setCopied(false);
    }
  }

  return (
    <button
      type="button"
      className="copy-btn"
      onClick={onClick}
      aria-label={copied ? 'Copied to clipboard' : 'Copy to clipboard'}
    >
      {copied ? 'Copied!' : label}
    </button>
  );
}
