/**
 * Pure helpers for the ExpertPicker (T3.5 — chat-ui expert picker).
 *
 * Kept free of React / network concerns so they can be unit-tested directly.
 * All functions return new arrays/objects — inputs are never mutated.
 */

import { ApiError } from '@xiaoguai/shared';
import type { ExpertSuggestion, Persona, Team } from '@xiaoguai/shared';

/** What is currently attached to the session (team takes display precedence). */
export interface ActiveExpert {
  kind: 'persona' | 'team';
  id: string;
  name: string;
}

/** Personas the picker offers: archived ones cannot be attached. */
export function selectablePersonas(personas: readonly Persona[]): Persona[] {
  return personas.filter((p) => !p.archived);
}

/** Teams the picker offers: archived ones cannot be attached. */
export function selectableTeams(teams: readonly Team[]): Team[] {
  return teams.filter((t) => !t.archived);
}

/**
 * Phase 4c — resolve the team a skill pack activated, by matching the pack
 * slug against each team's `recommended_pack_slugs`. Boot-scan stamps the
 * derived team with the pack's slug, so this is the deep-link bridge from a
 * Skills card ("Use in chat") to the team the chat picker should select.
 *
 * Returns the first non-archived match (teams are 1:1 with a pack in practice),
 * or `undefined` when nothing matches — the caller then falls back to a hint.
 */
export function teamForPackSlug(
  teams: readonly Team[],
  slug: string,
): Team | undefined {
  const wanted = slug.trim();
  if (!wanted) return undefined;
  return teams.find(
    (t) => !t.archived && t.recommended_pack_slugs.includes(wanted),
  );
}

/**
 * Phase 4c — render a namespaced member name (`slug/agent`) readably: drop the
 * pack-slug prefix and turn the agent id into a spaced, capitalized label.
 * A name without a `/` is title-cased as-is. Pure / never mutates.
 *
 *   "app-store-reviews/sentiment-analyst" → "Sentiment Analyst"
 *   "release_captain"                     → "Release Captain"
 */
export function readableMemberName(raw: string): string {
  const trimmed = raw.trim();
  if (!trimmed) return trimmed;
  const tail = trimmed.includes('/') ? trimmed.slice(trimmed.lastIndexOf('/') + 1) : trimmed;
  return tail
    .split(/[-_\s]+/)
    .filter((w) => w.length > 0)
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
    .join(' ');
}

/**
 * Case-insensitive substring filter on `name` (and `description` when the
 * item carries one). An empty / whitespace-only query keeps everything.
 */
export function filterByQuery<T extends { name: string; description?: string }>(
  items: readonly T[],
  query: string,
): T[] {
  const q = query.trim().toLowerCase();
  if (!q) return [...items];
  return items.filter(
    (item) =>
      item.name.toLowerCase().includes(q) ||
      (item.description ?? '').toLowerCase().includes(q),
  );
}

/** Highest score first; stable copy, never mutates the input. */
export function sortSuggestions(
  suggestions: readonly ExpertSuggestion[],
): ExpertSuggestion[] {
  return [...suggestions].sort((a, b) => b.score - a.score);
}

/** Compact score label: integers as-is, fractions to two decimals. */
export function formatScore(score: number): string {
  return Number.isInteger(score) ? String(score) : score.toFixed(2);
}

/**
 * True when the personas subsystem is not wired (HTTP 503) — the picker
 * hides itself entirely rather than showing dead controls.
 */
export function isExpertsUnavailable(err: unknown): boolean {
  return err instanceof ApiError && err.status === 503;
}

/**
 * True for "nothing attached" detach answers (HTTP 404). The remove action
 * fires both detach calls; whichever side wasn't attached is not an error.
 */
export function isNotAttached(err: unknown): boolean {
  return err instanceof ApiError && err.status === 404;
}
