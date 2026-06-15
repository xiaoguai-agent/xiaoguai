/**
 * PaneIntro — the standard "what is this pane / how do I use it" blurb that
 * sits directly under a pane's <h1>.
 *
 * Every admin pane should explain its **purpose** (what the feature is and why
 * it exists) and its **usage** (the concrete steps an operator takes), so the
 * UI never presents a bare table or empty form with no context. Some panes
 * also benefit from one or two worked **examples**.
 *
 * The component owns only presentation + structure; all copy is passed in
 * already-translated (callers use react-i18next's `t(...)`), keeping i18n key
 * ownership with each pane and this component free of translation coupling.
 *
 * Immutable by construction: props in, JSX out, no internal state.
 */
import type { ReactNode } from 'react';

export interface PaneIntroProps {
  /** One- or two-sentence statement of what the pane is for. Required. */
  readonly purpose: ReactNode;
  /**
   * How to use the pane — the steps/flow an operator follows. Optional so a
   * purely informational pane can omit it, but most panes should provide it.
   */
  readonly usage?: ReactNode;
  /**
   * Zero or more concrete examples. Empty/undefined renders nothing. Each
   * entry is rendered as its own line under an "Examples" label.
   */
  readonly examples?: readonly ReactNode[];
  /** Localized label for the usage line. Defaults to "How to use". */
  readonly usageLabel?: ReactNode;
  /** Localized label for the examples block. Defaults to "Examples". */
  readonly examplesLabel?: ReactNode;
}

/**
 * Render a pane's purpose/usage/examples intro block.
 *
 * Uses the lightweight `.pane-intro` class (defined in styles.css) which
 * reuses the existing `.hint` muted-text treatment and theme CSS variables.
 */
export function PaneIntro({
  purpose,
  usage,
  examples,
  usageLabel,
  examplesLabel,
}: PaneIntroProps): JSX.Element {
  const exampleList = (examples ?? []).filter(
    (e): e is ReactNode => e !== null && e !== undefined && e !== '',
  );

  return (
    <div className="pane-intro" role="note">
      <p className="pane-intro__purpose">{purpose}</p>
      {usage !== undefined && usage !== null && usage !== '' && (
        <p className="pane-intro__usage">
          {usageLabel !== undefined && usageLabel !== null && usageLabel !== '' && (
            <strong className="pane-intro__label">{usageLabel}</strong>
          )}{' '}
          {usage}
        </p>
      )}
      {exampleList.length > 0 && (
        <div className="pane-intro__examples">
          {examplesLabel !== undefined && examplesLabel !== null && examplesLabel !== '' && (
            <strong className="pane-intro__label">{examplesLabel}</strong>
          )}
          <ul>
            {exampleList.map((example, idx) => (
              <li key={idx}>{example}</li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
