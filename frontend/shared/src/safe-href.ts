/**
 * safeHref — protocol whitelist for externally-controlled link targets
 * (SEC-24 / SEC-25).
 *
 * RAG citation `source_uri`s, marketplace catalog `source_url`s, and the
 * per-tenant `link_to_disclosure` config all flow into `<a href>`. Those
 * values are backend/LLM-influenced, so a crafted `javascript:` / `data:` /
 * unknown-scheme URL there is an XSS or phishing primitive. This helper
 * parses the candidate with `new URL()` — which canonicalises scheme case,
 * strips embedded tab/newline tricks, and trims surrounding whitespace —
 * and only lets a fixed scheme whitelist through.
 *
 * Returns the original string when accepted (no normalisation, so existing
 * link shapes are preserved byte-for-byte) and `undefined` otherwise;
 * callers render plain text or omit the link on `undefined`.
 *
 * Relative URLs are rejected: every legitimate producer here emits absolute
 * URLs (the xiaoguai-rag citation lock guarantees `file:// | http(s):// |
 * obsidian:// | r2r://`), and refusing relatives avoids guessing a base.
 */

/**
 * Schemes allowed in externally-sourced hrefs. `obsidian:` and `r2r:` are
 * the custom schemes emitted by xiaoguai-rag citation URIs (see
 * chat-ui/src/citations.tsx contract comment).
 */
export const SAFE_HREF_PROTOCOLS: readonly string[] = [
  'http:',
  'https:',
  'file:',
  'mailto:',
  'obsidian:',
  'r2r:',
];

/**
 * Validate a candidate link target against the protocol whitelist.
 * Returns the input unchanged when safe; `undefined` when the URL is
 * missing, relative, unparseable, or uses a non-whitelisted scheme.
 */
export function safeHref(raw: string | undefined | null): string | undefined {
  if (raw === undefined || raw === null || raw === '') return undefined;
  let parsed: URL;
  try {
    parsed = new URL(raw);
  } catch {
    // Not an absolute URL (relative path, garbage, exotic syntax) —
    // refuse to linkify rather than guess a base (SEC-24).
    return undefined;
  }
  return SAFE_HREF_PROTOCOLS.includes(parsed.protocol) ? raw : undefined;
}
