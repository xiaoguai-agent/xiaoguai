/**
 * White-label assistant name. Fetched once from `GET /v1/branding` and shared
 * across every consumer via a module-level cache (so the logo, welcome line and
 * composer all agree without each issuing its own request). An empty owner
 * value means "use the locale's built-in default" — substitution happens at the
 * call site, which has the `t.ui.assistant_name` fallback.
 *
 * The name is read at page load; setting it in the admin UI (a separate SPA)
 * takes effect on the next chat-ui reload. That's deliberate — no polling.
 */
import { useEffect, useState } from 'react';
import { client } from './client';

/** Resolved owner name: `null` = not loaded yet, `''` = loaded but unset. */
let cache: string | null = null;
let inflight: Promise<string> | null = null;
const listeners = new Set<(name: string) => void>();

async function load(): Promise<string> {
  if (cache !== null) return cache;
  if (!inflight) {
    inflight = client
      .getBranding()
      .then((b) => (b.assistant_name ?? '').trim())
      .catch(() => ''); // backend unreachable / unauthed → built-in default
  }
  const name = await inflight;
  cache = name;
  listeners.forEach((l) => l(name));
  return name;
}

/**
 * The owner-configured assistant name, or `''` until it loads / when unset.
 * Callers fall back to their locale default: `useBrandName() || t.ui.assistant_name`.
 */
export function useBrandName(): string {
  const [name, setName] = useState(cache ?? '');
  useEffect(() => {
    listeners.add(setName);
    void load();
    return () => {
      listeners.delete(setName);
    };
  }, []);
  return name;
}
