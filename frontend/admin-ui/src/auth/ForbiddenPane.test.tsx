/**
 * Tests for ForbiddenPane (sprint-10b S10b-9).
 */
import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { I18nextProvider } from 'react-i18next';
import i18n from '../i18n/index';
import { ForbiddenPane } from './ForbiddenPane';

function renderPane(props: Parameters<typeof ForbiddenPane>[0]) {
  return render(
    <I18nextProvider i18n={i18n}>
      <ForbiddenPane {...props} />
    </I18nextProvider>,
  );
}

describe('ForbiddenPane', () => {
  it('renders title', () => {
    renderPane({});
    // Title is from i18n; should at minimum render a heading.
    const heading = screen.getByRole('heading', { level: 2 });
    expect(heading).toBeInTheDocument();
  });

  it('renders scope-specific message when scope provided', () => {
    renderPane({ scope: 'skill.approve' });
    // The i18n template interpolates {{scope}}; the rendered text should
    // contain the scope name verbatim somewhere.
    expect(screen.getByText(/skill\.approve/)).toBeInTheDocument();
  });

  it('renders generic message when scope absent', () => {
    renderPane({});
    // Heading still present; no scope name anywhere in the body.
    expect(screen.queryByText(/skill\.approve/)).not.toBeInTheDocument();
  });

  it('renders runbook link when URL provided', () => {
    renderPane({ scope: 'audit.export', runbookUrl: 'https://runbook.example.com/auth' });
    const link = screen.getByRole('link');
    expect(link).toHaveAttribute('href', 'https://runbook.example.com/auth');
    expect(link).toHaveAttribute('target', '_blank');
    expect(link).toHaveAttribute('rel', 'noopener noreferrer');
  });

  it('omits link when no runbook URL', () => {
    renderPane({});
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
  });

  it('marks itself as alert for screen readers', () => {
    renderPane({});
    expect(screen.getByRole('alert')).toBeInTheDocument();
  });
});
