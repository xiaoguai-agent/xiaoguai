/**
 * v0.8.2 — markdown rendering for assistant bubbles.
 *
 * react-markdown + remark-gfm covers tables, strikethrough, task lists,
 * autolinks. We deliberately don't enable raw HTML (it stays escaped) so
 * the assistant cannot inject script tags into the UI even if the model
 * is jailbroken into producing them.
 *
 * Code blocks render as monospace via the existing `.bubble.tool` token
 * stack so the visual rhythm of "prose vs. machine output" stays
 * consistent with the rest of the design pass.
 */

import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';

interface Props {
  text: string;
}

export function MarkdownBody({ text }: Props) {
  return (
    <div className="md">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        // Open links in a new tab so the chat doesn't navigate away.
        components={{
          a: ({ node: _node, ...props }) => (
            <a target="_blank" rel="noreferrer noopener" {...props} />
          ),
        }}
        // raw HTML stays escaped — security guardrail noted in the
        // module-level comment.
      >
        {text}
      </ReactMarkdown>
    </div>
  );
}
