/**
 * feat(single-owner-ux): audit-action categorisation helpers.
 *
 * Under the single-owner pivot the Audit pane is recast from an
 * enterprise compliance grid into a personal *activity history* — a
 * friendly, filterable, searchable list of "what I did and when".
 *
 * These pure helpers map a dotted audit `action` (e.g. `tool.invoke`,
 * `session.create`) to:
 *   - a coarse *category* key for the filter dropdown, derived from the
 *     action prefix (`action.split('.')[0]`), and
 *   - an i18n-safe label key (dots → underscores) so the table can show
 *     a human-readable verb with a fallback to the raw action.
 *
 * Kept side-effect free and dependency-free so they can be unit-tested
 * without rendering and reused by both the table and replay views.
 */

/** Coarse activity buckets surfaced in the filter dropdown. */
export const AUDIT_FILTER_CATEGORIES: string[] = [
  'all',
  'session',
  'tool',
  'auth',
  'memory',
  'approval',
  'code',
  'cost',
  'data',
  'other',
];

/**
 * Map an audit `action` to its category key, derived from the prefix
 * before the first dot. Unknown prefixes fall back to `'other'`.
 *
 * Examples: `session.create` → `session`, `git.commit` → `code`,
 * `hotl.escalate` → `approval`, `team.run` → `orchestration`.
 */
export function auditCategory(action: string): string {
  const prefix = action.split('.')[0] ?? '';
  switch (prefix) {
    case 'session':
      return 'session';
    case 'tool':
      return 'tool';
    case 'auth':
      return 'auth';
    case 'memory':
      return 'memory';
    case 'hotl':
      return 'approval';
    case 'code':
    case 'git':
      return 'code';
    case 'cost':
      return 'cost';
    case 'policy':
      return 'policy';
    case 'audit':
      return 'audit';
    case 'data':
      return 'data';
    case 'consent':
      return 'consent';
    case 'incident':
      return 'incident';
    case 'orchestration':
    case 'team':
    case 'loop':
      return 'orchestration';
    case 'skill':
      return 'skill';
    case 'agent':
      return 'agent';
    default:
      return 'other';
  }
}

/**
 * Convert an audit `action` into an i18n key fragment by replacing dots
 * with underscores, e.g. `session.create` → `session_create`. Used to
 * look up a friendly label under `pane.audit.actions.*` with the raw
 * action as the i18next `defaultValue` fallback.
 */
export function actionLabelKey(action: string): string {
  return action.replace(/\./g, '_');
}
