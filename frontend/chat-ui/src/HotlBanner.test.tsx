/**
 * HotlBanner — unit tests.
 *
 * Covers:
 *  - Renders with scope, reason, and escalation link when `pending` is supplied.
 *  - Banner has `role="alert"` (non-dismissible, screen-reader accessible).
 *  - Operator queue link encodes the escalation_id correctly.
 *  - Reason paragraph is omitted when `reason` is an empty string.
 */

import { render, screen } from '@testing-library/react';
import { describe, it, expect } from 'vitest';
import { HotlBanner } from './HotlBanner';
import type { HotlPendingState } from './HotlBanner';

const base: HotlPendingState = {
  escalation_id: 'esc-abc-123',
  scope: 'fs_write',
  reason: 'Policy rule: no filesystem writes outside /tmp.',
};

describe('HotlBanner', () => {
  it('renders the title and scope', () => {
    render(<HotlBanner pending={base} />);

    expect(screen.getByRole('alert')).toBeInTheDocument();
    expect(screen.getByText('Human approval required')).toBeInTheDocument();
    expect(screen.getByText('fs_write')).toBeInTheDocument();
  });

  it('renders the reason text', () => {
    render(<HotlBanner pending={base} />);

    expect(
      screen.getByText('Policy rule: no filesystem writes outside /tmp.'),
    ).toBeInTheDocument();
  });

  it('omits the reason paragraph when reason is empty', () => {
    const noBanner = { ...base, reason: '' };
    render(<HotlBanner pending={noBanner} />);

    // Reason paragraph should not be rendered.
    expect(
      screen.queryByText('Policy rule: no filesystem writes outside /tmp.'),
    ).toBeNull();
  });

  it('renders a link to the operator approval queue with encoded escalation_id', () => {
    render(<HotlBanner pending={base} />);

    const link = screen.getByRole('link', { name: /open operator approval queue/i });
    expect(link).toBeInTheDocument();
    expect(link).toHaveAttribute(
      'href',
      '/hotl-queue?escalation_id=esc-abc-123',
    );
    expect(link).toHaveAttribute('target', '_blank');
    expect(link).toHaveAttribute('rel', 'noopener noreferrer');
  });

  it('prepends adminBaseUrl to the queue link', () => {
    render(<HotlBanner pending={base} adminBaseUrl="https://admin.example.com" />);

    const link = screen.getByRole('link', { name: /open operator approval queue/i });
    expect(link).toHaveAttribute(
      'href',
      'https://admin.example.com/hotl-queue?escalation_id=esc-abc-123',
    );
  });

  it('encodes special characters in escalation_id', () => {
    const specialId: HotlPendingState = { ...base, escalation_id: 'esc a+b=c&d' };
    render(<HotlBanner pending={specialId} />);

    const link = screen.getByRole('link', { name: /open operator approval queue/i });
    // encodeURIComponent('esc a+b=c&d') = 'esc%20a%2Bb%3Dc%26d'
    expect(link.getAttribute('href')).toContain('esc%20a%2Bb%3Dc%26d');
  });
});
