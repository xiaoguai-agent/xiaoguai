/**
 * v0.8.2 — markdown rendering for assistant bubbles.
 * v0.8.3 — fenced code blocks route through `CodeBlock` (Prism async-light
 * + copy button). Inline code keeps the simple `.md code` styling.
 *
 * react-markdown + remark-gfm covers tables, strikethrough, task lists,
 * autolinks. Raw HTML stays escaped (the default) so the assistant cannot
 * inject script tags even if jailbroken into producing them.
 */

import ReactMarkdown, { type Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { CodeBlock } from './codeblock';

interface Props {
  text: string;
}

const components: Components = {
  a: ({ node: _node, ...props }) => (
    <a target="_blank" rel="noreferrer noopener" {...props} />
  ),
  // react-markdown v10 dropped the `inline` prop; we distinguish inline
  // from fenced by the `language-*` className it sets on fenced blocks
  // *or* by the presence of a newline in the value (untagged fences).
  // Anything that smells like a one-liner with no language is inline
  // code and stays under the cheap `.md code` styling.
  code: ({ className, children, node: _node, ...rest }) => {
    const value = String(children ?? '').replace(/\n$/, '');
    const langMatch = /language-([\w-]+)/.exec(className ?? '');
    const isFence = langMatch !== null || value.includes('\n');
    if (!isFence) {
      return (
        <code className={className} {...rest}>
          {children}
        </code>
      );
    }
    return <CodeBlock language={langMatch?.[1]} value={value} />;
  },
  // react-markdown wraps fenced code in `<pre><code>`. Since `CodeBlock`
  // already renders its own `<pre>`, strip the outer one to avoid nested
  // <pre> tags.
  pre: ({ children }) => <>{children}</>,
};

export function MarkdownBody({ text }: Props) {
  return (
    <div className="md">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  );
}
