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
