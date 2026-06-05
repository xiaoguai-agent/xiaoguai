import { useTranslation } from 'react-i18next';
import type { AuditEntryView } from '@xiaoguai/shared';
import { ChainBadge } from './ChainBadge';

/**
 * P1.5 (DEC-037) — replay viewer. Renders the signed audit rows as a
 * step-by-step run timeline (one entry per audited action), the human-readable
 * complement to the `xiaoguai audit bundle` JSON. Coding-workflow actions are
 * emphasised and their checkpoint id is surfaced, so a reviewer can walk
 * "what the agent did, and how to revert each step" straight from the chain.
 *
 * Presentational only: it renders the SAME rows the Audit pane already loads
 * via `listAudit`; chain integrity is shown per step via `<ChainBadge>`.
 */
export interface AuditReplayProps {
  rows: AuditEntryView[];
}

/** Governed coding actions (LLD-CODING-001 §2) that get visual emphasis. */
const CODING_ACTIONS = new Set([
  'code.edit',
  'code.run',
  'git.commit',
  'git.push',
  'pr.open',
  'code.rollback',
]);

/** Pull the checkpoint id out of a row's `details` JSON, if present. */
export function checkpointOf(details: unknown): string | null {
  if (details && typeof details === 'object' && 'checkpoint' in details) {
    const cp = (details as Record<string, unknown>).checkpoint;
    return typeof cp === 'string' && cp.length > 0 ? cp : null;
  }
  return null;
}

export function AuditReplay({ rows }: AuditReplayProps): JSX.Element {
  const { t } = useTranslation();
  return (
    <ol className="audit-replay" data-testid="audit-replay">
      {rows.map((r, i) => {
        const cp = checkpointOf(r.details);
        const coding = CODING_ACTIONS.has(r.action);
        return (
          <li key={r.id} className={coding ? 'replay-step coding' : 'replay-step'}>
            <div className="replay-head">
              <span className="replay-step-n">#{r.id}</span>
              <span className="replay-action">{r.action}</span>
              <ChainBadge entry={r} prevEntry={rows[i - 1]} />
            </div>
            <div className="replay-meta">
              <time>{new Date(r.ts).toLocaleString()}</time>
              <span className="replay-actor">{r.actor}</span>
              {r.resource && <span className="replay-resource">{r.resource}</span>}
              {cp && (
                <span className="replay-checkpoint" data-testid="replay-checkpoint">
                  {t('pane.audit.replay_checkpoint')}: <code>{cp.slice(0, 8)}</code>
                </span>
              )}
            </div>
          </li>
        );
      })}
    </ol>
  );
}
