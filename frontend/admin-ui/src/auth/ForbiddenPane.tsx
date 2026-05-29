/**
 * Sprint-10b S10b-9 — Forbidden fallback pane (REQ-UI-006).
 *
 * Rendered when a pane's data fetch returns 403, after `<RequireScope>` failed
 * open (e.g. backend doesn't expose `/v1/admin/me/scopes`). Tells the operator
 * which scope is missing and points at the runbook.
 *
 * Not a substitute for backend Casbin enforcement — this is UX after the fact.
 */

import { useTranslation } from 'react-i18next';

export interface ForbiddenPaneProps {
  /** The Casbin scope name the action required (e.g. `skill.approve`). */
  scope?: string;
  /** Optional runbook URL to link the operator to. */
  runbookUrl?: string;
}

export function ForbiddenPane({ scope, runbookUrl }: ForbiddenPaneProps): JSX.Element {
  const { t } = useTranslation();
  return (
    <div className="forbidden-pane" role="alert" aria-live="polite">
      <h2>{t('auth.forbiddenTitle')}</h2>
      {scope ? (
        <p>
          {t('auth.forbiddenScopeMessage', { scope })}
        </p>
      ) : (
        <p>{t('auth.forbiddenGenericMessage')}</p>
      )}
      {runbookUrl && (
        <p>
          <a href={runbookUrl} target="_blank" rel="noopener noreferrer">
            {t('auth.forbiddenRunbookLink')}
          </a>
        </p>
      )}
    </div>
  );
}
