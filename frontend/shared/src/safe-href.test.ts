/**
 * safeHref (SEC-24 / SEC-25) — protocol whitelist for externally-controlled
 * link targets. Active schemes (javascript:, data:, …) must never reach an
 * `<a href>`; the documented citation schemes must pass unchanged.
 */

import { describe, expect, it } from 'vitest';
import { safeHref, SAFE_HREF_PROTOCOLS } from './safe-href';

describe('safeHref', () => {
  it('rejects javascript: URLs', () => {
    expect(safeHref('javascript:alert(1)')).toBeUndefined();
  });

  it('rejects javascript: regardless of case or surrounding whitespace', () => {
    // `new URL()` lowercases the scheme and trims whitespace, so these
    // obfuscations must not slip past the whitelist.
    expect(safeHref('JaVaScRiPt:alert(1)')).toBeUndefined();
    expect(safeHref('  javascript:alert(1)')).toBeUndefined();
    expect(safeHref('java\tscript:alert(1)')).toBeUndefined();
  });

  it('rejects data: URLs', () => {
    expect(safeHref('data:text/html,<script>alert(1)</script>')).toBeUndefined();
  });

  it('rejects other non-whitelisted schemes', () => {
    expect(safeHref('vbscript:msgbox(1)')).toBeUndefined();
    expect(safeHref('blob:https://example.com/uuid')).toBeUndefined();
    expect(safeHref('intent://scan/#Intent;end')).toBeUndefined();
  });

  it('passes http and https URLs through unchanged', () => {
    expect(safeHref('https://x')).toBe('https://x');
    expect(safeHref('https://github.com/foo/bar/blob/main/src/lib.rs#L1-L9')).toBe(
      'https://github.com/foo/bar/blob/main/src/lib.rs#L1-L9',
    );
    expect(safeHref('http://localhost:7600/docs')).toBe('http://localhost:7600/docs');
  });

  it('passes the citation-lock schemes (file, obsidian, r2r)', () => {
    expect(safeHref('file:///home/owner/notes.md')).toBe('file:///home/owner/notes.md');
    expect(safeHref('obsidian://open?vault=main&file=note')).toBe(
      'obsidian://open?vault=main&file=note',
    );
    expect(safeHref('r2r://documents/abc123')).toBe('r2r://documents/abc123');
  });

  it('passes mailto links', () => {
    expect(safeHref('mailto:owner@example.com')).toBe('mailto:owner@example.com');
  });

  it('rejects relative URLs (no base guessing)', () => {
    expect(safeHref('/docs/disclosure')).toBeUndefined();
    expect(safeHref('../up/one')).toBeUndefined();
    expect(safeHref('plain-text-not-a-url')).toBeUndefined();
  });

  it('rejects empty / null / undefined', () => {
    expect(safeHref('')).toBeUndefined();
    expect(safeHref(null)).toBeUndefined();
    expect(safeHref(undefined)).toBeUndefined();
  });

  it('whitelist contains only the expected schemes', () => {
    expect([...SAFE_HREF_PROTOCOLS].sort()).toEqual(
      ['file:', 'http:', 'https:', 'mailto:', 'obsidian:', 'r2r:'].sort(),
    );
  });
});
