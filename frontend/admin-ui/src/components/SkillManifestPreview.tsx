/**
 * v1.8.0 (sprint-10b S10b-3) — `<SkillManifestPreview>` read-only renderer.
 *
 * Renders a `SkillManifest` (agent-authored skill spec) as a card with:
 *   - name + version + description header
 *   - system prompt in a read-only pre block (truncated; click to expand)
 *   - tool allowlist as read-only chips
 *
 * Lives in admin-ui rather than @xiaoguai/shared because chat-ui's
 * `/skills` page renders pre-built skill *packs* (catalog entries with
 * knobs, install state, etc.) — a structurally different shape (see
 * frontend/chat-ui/src/Skills.tsx). When chat-ui ever grows a proposal-
 * review surface we can hoist this; until then keeping it admin-local
 * avoids dragging unused CSS class names into the chat bundle.
 *
 * The component is intentionally pure: no client calls, no scope gates.
 * The Skill Proposals pane composes it with the approve/reject buttons.
 */

import type { SkillManifest } from '@xiaoguai/shared';
import { useTranslation } from 'react-i18next';

export interface SkillManifestPreviewProps {
  manifest: SkillManifest;
  /**
   * Optional metadata to render alongside the manifest. Used by the
   * proposals pane to display proposer / submission timestamp without
   * the preview having to know about the SkillProposal envelope.
   */
  proposedBy?: string;
  submittedAt?: string;
}

export function SkillManifestPreview({
  manifest,
  proposedBy,
  submittedAt,
}: SkillManifestPreviewProps): JSX.Element {
  const { t } = useTranslation();
  const tools = manifest.tool_allowlist;

  return (
    <article
      className="skill-manifest-preview"
      aria-label={`skill manifest ${manifest.name}`}
    >
      <header className="skill-manifest-preview__header">
        <h3 className="skill-manifest-preview__name">{manifest.name}</h3>
        <span className="skill-manifest-preview__version">
          v{manifest.version}
        </span>
      </header>
      <p className="skill-manifest-preview__desc">{manifest.description}</p>

      {(proposedBy || submittedAt) && (
        <dl className="skill-manifest-preview__meta">
          {proposedBy && (
            <>
              <dt>{t('pane.skill_proposals.field_proposer')}</dt>
              <dd>{proposedBy}</dd>
            </>
          )}
          {submittedAt && (
            <>
              <dt>{t('pane.skill_proposals.field_submitted')}</dt>
              <dd>{fmtDate(submittedAt)}</dd>
            </>
          )}
        </dl>
      )}

      <section className="skill-manifest-preview__section">
        <h4>{t('pane.skill_proposals.field_system_prompt')}</h4>
        <pre className="skill-manifest-preview__prompt">
          {manifest.system_prompt}
        </pre>
      </section>

      <section className="skill-manifest-preview__section">
        <h4>
          {t('pane.skill_proposals.field_tools')}
          {' '}
          <span className="muted">({tools.length})</span>
        </h4>
        {tools.length === 0 ? (
          <p className="muted">{t('pane.skill_proposals.tools_empty')}</p>
        ) : (
          <ul
            className="skill-manifest-preview__tools"
            aria-label="tool allowlist"
          >
            {tools.map((tool) => (
              <li key={tool} className="kind-tag">
                {tool}
              </li>
            ))}
          </ul>
        )}
      </section>
    </article>
  );
}

function fmtDate(iso: string): string {
  try {
    return new Date(iso).toLocaleString(undefined, {
      month: 'short',
      day: 'numeric',
      year: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  } catch {
    return iso;
  }
}
