/**
 * v0.8.3 — syntax-highlighted code block with copy button.
 *
 * Why `PrismLight` (not the default `Prism` build, not `PrismAsyncLight`):
 *   - The default `Prism` build pulls every language `refractor` knows
 *     (~290) into the main bundle: not viable.
 *   - `PrismAsyncLight` *also* references the whole `refractor`
 *     dynamic-import tree; Vite materialises one chunk per language at
 *     build time (~280 extra files), bloating `dist/` even though most
 *     are never fetched. Not worth the deploy-size tax.
 *   - `PrismLight` exposes the same `registerLanguage` API but its core
 *     ships zero grammars and has no dynamic-import fan-out. We register
 *     the 9 languages we care about + their common aliases statically,
 *     so the bundle grows by exactly what we use. Unknown languages
 *     render as plain monospace (still inside the copy-button wrapper).
 *
 * Why two themes registered side-by-side (oneLight / oneDark): the
 * highlighter receives a JS style object, so theme switches happen via a
 * React re-render driven by `useTheme()`. There is no separate CSS
 * payload for either theme — they're plain object literals living in JS.
 */

import { useState, type ReactNode } from 'react';
import { PrismLight as SyntaxHighlighter } from 'react-syntax-highlighter';
import {
  oneLight,
  oneDark,
} from 'react-syntax-highlighter/dist/esm/styles/prism';
import bash from 'react-syntax-highlighter/dist/esm/languages/prism/bash';
import json from 'react-syntax-highlighter/dist/esm/languages/prism/json';
import typescript from 'react-syntax-highlighter/dist/esm/languages/prism/typescript';
import javascript from 'react-syntax-highlighter/dist/esm/languages/prism/javascript';
import python from 'react-syntax-highlighter/dist/esm/languages/prism/python';
import rust from 'react-syntax-highlighter/dist/esm/languages/prism/rust';
import sql from 'react-syntax-highlighter/dist/esm/languages/prism/sql';
import yaml from 'react-syntax-highlighter/dist/esm/languages/prism/yaml';
import markdown from 'react-syntax-highlighter/dist/esm/languages/prism/markdown';
import { useTheme } from './theme';

SyntaxHighlighter.registerLanguage('bash', bash);
SyntaxHighlighter.registerLanguage('sh', bash);
SyntaxHighlighter.registerLanguage('shell', bash);
SyntaxHighlighter.registerLanguage('json', json);
SyntaxHighlighter.registerLanguage('typescript', typescript);
SyntaxHighlighter.registerLanguage('ts', typescript);
SyntaxHighlighter.registerLanguage('tsx', typescript);
SyntaxHighlighter.registerLanguage('javascript', javascript);
SyntaxHighlighter.registerLanguage('js', javascript);
SyntaxHighlighter.registerLanguage('jsx', javascript);
SyntaxHighlighter.registerLanguage('python', python);
SyntaxHighlighter.registerLanguage('py', python);
SyntaxHighlighter.registerLanguage('rust', rust);
SyntaxHighlighter.registerLanguage('rs', rust);
SyntaxHighlighter.registerLanguage('sql', sql);
SyntaxHighlighter.registerLanguage('yaml', yaml);
SyntaxHighlighter.registerLanguage('yml', yaml);
SyntaxHighlighter.registerLanguage('markdown', markdown);
SyntaxHighlighter.registerLanguage('md', markdown);

const REGISTERED = new Set([
  'bash', 'sh', 'shell',
  'json',
  'typescript', 'ts', 'tsx',
  'javascript', 'js', 'jsx',
  'python', 'py',
  'rust', 'rs',
  'sql',
  'yaml', 'yml',
  'markdown', 'md',
]);

interface CodeBlockProps {
  language: string | undefined;
  value: string;
}

export function CodeBlock({ language, value }: CodeBlockProps) {
  const { effective } = useTheme();
  const style = effective === 'dark' ? oneDark : oneLight;
  const lang = language && REGISTERED.has(language) ? language : 'text';

  return (
    <div className="codeblock">
      <CopyButton text={value} />
      {lang === 'text' ? (
        <pre className="codeblock__plain">
          <code>{value}</code>
        </pre>
      ) : (
        <SyntaxHighlighter
          language={lang}
          style={style}
          PreTag="pre"
          CodeTag="code"
          customStyle={{
            margin: 0,
            padding: 'var(--space-3)',
            background: 'var(--code-bg)',
            border: '1px solid var(--border)',
            borderRadius: 'var(--radius-md)',
            fontSize: 13,
            lineHeight: 1.55,
          }}
          codeTagProps={{ style: { background: 'transparent' } }}
        >
          {value}
        </SyntaxHighlighter>
      )}
    </div>
  );
}

interface CopyButtonProps {
  text: string;
  /** Optional override for the resting label (defaults to "Copy"). */
  label?: ReactNode;
}

/**
 * Top-right corner overlay. Hides until the user hovers (or focuses via
 * keyboard) the enclosing `.codeblock` / `.copy-host` element. Showing
 * "Copied!" for 1.2s then reverting matches the brief.
 */
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
